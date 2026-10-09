# crosscheck: ac2 against REW and steady-sine truth

An on-demand suite, not run in CI. It measures the same paths with ac2, with REW (offline
import of a recording), with steady sines analysed in numpy, and from the raw recordings
themselves. Then it compares the numbers against the tolerances in `tolerances.toml`. It
reruns the hand comparison of 2026-10-07 (`docs/rigs/pupu.md`, "REW cross-check, electrical")
and adds an ambient SPL check and the speaker path.

```
crosscheck/      the package (python -m crosscheck {preflight,run,analyse,baseline,compare})
rigs/pupu.toml   ports, roles, paths, levels, stage settings for pupu
tolerances.toml  PASS / WARN limits for every comparison, compare steps
baselines/       reviewed results per rig, stage and level (<rig>/<stage>-<level>dbfs.json)
reference/       documented truth of 2026-10-07 (used when analysing the fixtures)
rig-run.sh       dev host: copy to the rig, run there, fetch the run, analyse and compare here
tests/           pytest: dsp, safety policy, JACK dummy server, fixtures smoke test, digital DUT
```

## Prerequisites

The **rig** (pupu) runs `run` and `preflight`. It needs only numpy and JACK-Client:
`~/ac2-test/venv-meas/bin/python` has both. scipy and matplotlib are not needed there.

- jackd at 96 kHz with the FF400. ac2d runs as the systemd user unit `ac2d.service` with
  `--max-level -50`, and its session is open or can be opened.
- REW 5.40 on Xvfb with its API on `localhost:4735` (`docs/rigs/pupu.md`). The REW stages
  import recordings, so no Pro licence is needed. Without REW they are skipped.
- PipeWire with the `pw_rew` tunnel, for the ambient stage only. If PipeWire is not running,
  the stage starts it and stops it again afterwards.
- **ac2 CLI paired with the local daemon (one-time).** The rig's own CLI is not paired with
  localhost yet:
  1. Run `journalctl --user -u ac2d | grep -i key` to read the daemon key ac2d logs at startup.
  2. Run `~/ac2-test/bin/ac2 auth pair localhost --server-key <key>`.
  3. Authorize the new client from the app on ketunkolo (Settings → Connection: **A** on the
     refused key, give it a name).

  Alternatively, set `[ac2].cmd` in the rig file to a paired host over ssh. pupu has no ssh
  key for ketunkolo today.

The **dev host** runs `analyse`. It needs numpy; matplotlib adds the plots and pytest runs
the tests. scipy is not used.

## The REW stimulus

REW's sweep is played by the suite's own JACK client and imported with the recording, so the
stimulus is a WAV file REW made. It must use the rig's rate (96 kHz).

- **Electrical path:** `~/rew-dl/sweep96k.wav` is the existing file (10 Hz – 40 kHz, 512k,
  peak −50 dBFS). Before playing, the suite scales it so its peak is the `--emit` level.
- **Speaker path:** make a 20 Hz – 20 kHz file. In REW, open Generator → Measurement sweep,
  set Start 20 Hz, End 20 kHz, Length 512k and level −50 dBFS, then save it as
  `~/rew-dl/sweep96k-20-20k.wav` (`[rew].stimulus_speaker`). The suite estimates where the
  file's sweep starts and refuses a file that starts below 90 % of `speaker_min_request_hz`
  (20 Hz). It never scales this file up, and it refuses a peak above −46 dBFS.

## Running it

From the dev host, in `tools/crosscheck`. The **default run** has all stages at −50 dBFS
(ambient, Genelec, Xone):

```
./rig-run.sh --emit -50dbfs --emit-speaker -50dbfs
```

The **electrical −10 dBFS** run has the Xone stages at −10 and the Genelec stages still at −50.
Use it for judging: at −50 the electrical comparisons below 1 kHz are noise-limited (group
delay and harmonics INCONCLUSIVE or failing on scatter), and no ear is on these paths:

```
./rig-run.sh --emit -10dbfs --allow-electrical-level -10dbfs --emit-speaker -50dbfs
```

The pieces on their own:

```
ssh -t mui@192.168.9.27 'cd ~/crosscheck && ~/ac2-test/venv-meas/bin/python -m crosscheck preflight --rig rigs/pupu.toml'
./rig-run.sh --stages ambient                                     # silent
./rig-run.sh --stages xone --emit -50dbfs                         # electrical only, no speaker
./rig-run.sh --stages genelec --emit-speaker -50dbfs --skip ac2_tf
python -m crosscheck analyse runs/<UTC time>                      # again, after changing tolerances
python -m crosscheck analyse /work/ac2-scratch/crosscheck-fixtures --out /tmp/xc-fixtures
```

`rig-run.sh` copies the package to `~/crosscheck` on the rig and runs it under `ssh -t`, so
the operator can confirm and press Ctrl-C there. It then copies `runs/<UTC time>` back to
`/work/ac2-crosscheck/runs` when that directory exists, else `runs/` here (git-ignored; set
`CROSSCHECK_RUNS` to change that), writes `runs/<UTC time>/report/` and compares the run with
the baselines (below); differences are printed, they do not fail the script.

### The Genelec stage is audible: the operator must be present

Out 1 drives the Genelec 1083. At −50 dBFS a 1 kHz sine gives about **63 dB SPL at the mic**
(83 at −30 with `--allow-speaker-level`):
−30 dBFS gave 83.2 dB SPL on 2026-10-03 (`docs/rigs/pupu.md`), and 20 dB less is about 63.
Sweeps and pink noise at −50 dBFS are in the same range. The stage stops and waits for Enter
(`--yes` skips the wait; without a terminal and without `--yes` it is refused). Stay within
reach of Ctrl-C.

### Levels and safety

- **Units are required.** Every level is typed with its unit (`-50dbfs`); a bare number is
  refused. The levels follow the full-scale-sine dBFS convention of ac2 and REW.
- **Nothing emits without a flag.** `--emit` covers the electrical-only outputs (out 3 Xone,
  out 2 loopback). `--emit-speaker` covers the speaker stage, which plays out 1 plus the
  loopback.
- **The speaker stays at or below −50 dBFS** whatever the config or the electrical flags say
  (`levels.SPEAKER_HARD_MAX_DBFS`). Only the operator, present at the rig, may lift it with
  `--allow-speaker-level`, and never above −30 dBFS (`levels.SPEAKER_APPROVED_MAX_DBFS`,
  about 83 dB SPL at the mic for a 1 kHz sine). That flag raises ac2d's bound through the
  same drop-in for the speaker stage only and removes it before the next stage:
  `./rig-run.sh --stages genelec --emit-speaker -30dbfs --allow-speaker-level -30dbfs`.
  REW's stimulus file is scaled to the stage's level like every other signal. The Xone
  stages refuse out 1 (`forbidden_outputs`).
- **Above the rig's −50 needs `--allow-electrical-level`**, capped by the config
  (`electrical_max_dbfs`, −10 on pupu) and at −6 in code (`levels.ELECTRICAL_HARD_MAX_DBFS`).
  That flag installs a runtime systemd drop-in, restarts ac2d with `--max-level <level>` and
  reopens the session. This happens only **after** the speaker stage.
  In a `finally` (also on Ctrl-C, SIGTERM or SIGHUP) the drop-in is removed, ac2d restarts,
  and `gen ceiling` must read a bound of −50 again. If it does not, the run says so loudly
  and exits 3. A drop-in left over from a crash is removed before anything else runs.
- **Before the speaker stage**, the suite checks that ac2d's bound and ceiling read −50.
- **Signals the suite plays itself** (sines, REW's stimulus) are checked against the
  permitted peak before JACK starts. They are faded in and out (raised cosine) and clipped to
  that peak in the callback. Ctrl-C fades the outputs out over the fade time before the
  client stops.
- **ac2's own stimuli** (`sweep run`, `gen pink`) go through ac2d's ceiling. The suite stops
  them with `q` and then `gen stop`, and ac2d fades them out. On the speaker path:
  - ac2's sweep must start at or above `speaker_min_emit_hz` (5 Hz), as ac2 reports it after
    `meas new sweep`. The suite asks for 20 Hz.
  - Pink noise is high-passed at 20 Hz.
- **Speaker low-frequency extension.** ac2's sweep starts at the requested frequency under a
  fade-in (about 92 ms for 20 Hz / 5.5 s). Below the start it emits only the fade's leakage.
  The 1083's own roll-off is below that, so 20 Hz is the lowest frequency asked of it.

### Order and what each stage does

1. **ambient** (silent, about 60 s): input 1 measured at the same time by
   - ac2's SPL meters (Z, A and C, slow, `spl watch --for`),
   - REW's SPL meters 1–3 (Z/A/C) and RTA through PipeWire, with `system:capture_1 → pw_rew:capture_FL`
     wired for the stage and the previous wiring restored afterwards,
   - ac2's third-octave RTA,
   - a JACK recording analysed in numpy (FFT, analytic A/C weighting, the mic curve).
2. **genelec** at −50 dBFS: out 1 → Genelec → mic on in 1; reference out 2 → in 2.
3. **xone** at `--emit`: out 5 → Xone (L) → in 5; reference out 2 → in 2. With
   `--allow-electrical-level`, the drop-in is installed just before this stage.
4. **dut** at `--emit` (opt-in: `--stages …,dut`): the digital DUT (below). Same flags and
   level policy as xone; the drop-in, if any, stays from the xone stage.

Each path stage runs these sub-stages (`--skip` takes their names):

- **sine**:
  - A silent take, then a short probe per tone. The lengths are then planned so the
    harmonic floor reaches `target_floor_dbr` within `budget_seconds`: a floor in a fixed
    window lobe falls 10·log10(T).
  - The final takes, then a silent take long enough for every tone.
  - Each tone is played on the measurement and reference outputs. The results are the
    meas÷ref phasor (`ratio_db` and `ratio_deg`), the levels, and H2–H5 with their floors.
  - On the Xone path, group delay comes from extra tones at f·2^(±1/48).
  - Frequencies are nudged (by up to 6 %) so that H2–H5 stay clear of 50 Hz multiples by
    the main lobe + 2 bins.
- **rew**:
  - The stimulus is played (and repeated if `[stages.rew].repeats` > 1, then averaged
    sample-synchronously) and recorded on both inputs.
  - Each channel is imported into REW against the stimulus: the mic with `applyCal`, the
    others uncalibrated. Exports: FR, IR, GD, distortion, RT60.
  - The suite's REW measurements are deleted afterwards.
- **ac2_sweep**: the variants in the rig file. `plan = true` runs a probe, reads the H2 floor
  over `floor_band_hz`, then picks repeats (+10·log10 N) and length (+10·log10 T) to reach
  the target within the budget. `plan.json` gives the predicted and the achieved floor. The
  daemon also records each variant raw (`rec start`), starting one sweep length plus 1 s
  early so that noise windows exist.
- **ac2_tf**: pink noise for `settle_seconds`, a captured trace and a raw recording.

### Digital DUT path

`ac2-jack-dut` is a JACK client with exactly known harmonics and no mains:
`dut_out = post(poly(pre(dut_in))) + noise`, `ref_out = ref_in + noise` (`[dut]` in the rig
file: polynomial, pre/post filter specs, noise −120 dBFS). The suite designs the biquads
(`crosscheck/dut.py`), starts the binary for the path's stages, checks its rate and that its
inputs are unconnected, writes `dut/dut.json` (the exact command, the coefficients, the
analytic truth for the tones and a 1/12-octave grid) and stops it in a finally (stdin closed,
then SIGTERM, then kill), keeping its `xruns <n>` line.

- **What it proves:** whether the analysers read a harmonic right. Each "vs analytic" row
  (group **dut**) compares a reading with the exact truth at the column's own frequency: the
  steady sine (the method check, 0.1 / 0.3 dB), REW's offline import and each ac2 sweep
  variant (0.5 / 1.0 dB, per tone and as a log-grid summary; `[dut]` in `tolerances.toml`).
  The default polynomial gives H2 −50, H3 −55, H4 −63, H5 −70 dBr at −10 dBFS and 1 kHz,
  falling at LF where the 20 Hz pre-filter lowers its input (15 Hz: −62 … −88). On the Xone
  those harmonics sit at the converters' floor, so most comparisons there are INCONCLUSIVE.
- **What it does not prove:** anything about the rig's analogue path. It tests the
  analysers, not the rig.
- **Wiener vs Hammerstein:** the pre-filter sits ahead of the polynomial. A steady sine's
  harmonics are then exactly the analytic truth; a sweep sees the pre-filter only in the
  instantaneous-frequency approximation. Rows where the pre-filter's gain exceeds 0.1 dB say
  so: a deviation there can be the model, not the analyser. The post-filter has no such caveat.
- **Ports:** the suite's own player and recorder use `[paths.dut].ports` (the DUT's ports). For
  the ac2 stages the suite records what feeds `ac2:in_<meas_in>` and `ac2:in_<ref_in>`
  (3 and 2), disconnects it, patches `ac2-dut:dut_out → ac2:in_3`, `ac2-dut:ref_out → ac2:in_2`,
  `ac2:out_5 → ac2-dut:dut_in`, `ac2:out_2 → ac2-dut:ref_in`, and checks before every sweep that
  each patched input has the DUT as its only source. Afterwards (in a finally) it removes
  exactly those links and restores the recorded ones. If that fails it prints `!!!` lines
  naming `ac2 session open …` (the rig's `[ac2].session_open`) and the run exits 4.
- **Hardware outputs still play.** ac2d connects its outputs to `system:playback_N` itself,
  so during the ac2 stages outs 5 and 2 also carry ac2's stimulus at `--emit`. Both are
  approved electrical/loopback outputs, and the level policy checks them as on the Xone path
  (out 1 is refused).
- **xruns:** on an xrun JACK leaves a stale block in `dut_out`. A suite take with an xrun is
  discarded as on every path (`max_xruns`, server-wide notifications). The DUT's own count
  appears as a "DUT xruns" row, WARN when it is not 0 (the path's results are then suspect).
- **Deploy:** build `ac2-jack-dut` (crate `tools/jack-dut`) for the rig and copy it to the
  rig's `~/ac2-test/bin/ac2-jack-dut` (`[dut].binary`). `preflight` warns when it is
  missing, and `run --stages dut` refuses to start without it. `AC2_JACK_DUT` overrides the path.
- **Locally:** `CROSSCHECK_JACK_E2E=1 AC2_JACK_DUT=<binary> pytest tests/test_dut_e2e.py` runs
  the sine stage and the patch against the real binary on a JACK dummy server. There is no
  ac2 daemon in that test.

### Calibration (speaker path and ambient)

The suite reads ac2's store (`~/.config/ac2/calibrations.json`) and derives REW's input
calibration from it:

- `dBFSAt94dBSPL = 94 − sensitivity`
- `fullScaleSineVrms` = the store's "0 dBFS = X V"
- `calFilePath` = the active mic curve, shifted to 0 dB at 1 kHz as ac2 applies it

It checks the convention against the mic's stated mV/Pa and refuses a mismatch over 0.5 dB.
REW's calibration is read at the start (`cal/rew_input_cal.before.json`) and put back at the
end, with the SPL-meter and RTA settings. The report shows the mapping table. The Xone stages
are uncalibrated ratio comparisons.

The curve is in some columns and not in others: ac2's live TF carries it ("in the columns"
in its export), its sweeps do not (ac2 applies the input's curve to a sweep after capture, as
a display edit: "applied after capture …, not in the columns"), REW's speaker import
carries it, and the direct cross-spectra and steady sines are of the raw inputs. The suite
takes the curve back out of the TF and of REW's dBFS response when it loads a run, so every
relative comparison is of raw inputs; only the absolute-SPL rows put it back, for every
source alike.

## Reading the report

`report/report.md` starts with the counts and the failures and warnings. Then come the
calibration mapping, one table of checks per group, the data tables and the plots.
`results.json` holds the same data, machine-readable.

- **PASS / WARN / FAIL** compare `|value|` with (pass, warn) from `tolerances.toml`. Each row
  says what was compared with what.
- **INCONCLUSIVE** means the data cannot tell a pass from a fail:
  - a reading below floor + `margin_db` (10 dB). Such a reading is an upper bound "< X",
    never a value, and the row gives the shortfall. Comparisons that rest on bounds never
    PASS or FAIL. One exception: a value more than the warn limit above a bound is a FAIL.
  - a comparison whose own uncertainty (2σ: probe floors, phase scatter, a band's noise from
    the recording's SNR, a room's fine structure) is wider than the pass limit. It FAILs only
    when it misses the warn limit by more than that uncertainty.
  - a truth or reading a mains line sits in (the sine GD pair, a sweep's harmonic band).
- **INFO** rows are context, not judged: REW's offline delays (gone by design), REW's own GD
  export, REW's RTA averaging, an electrical path's ETC skirts.
- **One phase reference**: meas ÷ ref with the path's delay in it. ac2's reported arrival is
  put back into its phase, and REW offline gets back the delay its timing markers removed
  (from the direct estimate of the same recording).
- **direct** sources are numpy cross-spectra of the same recording an app analysed. A
  difference to them is the app's analysis, not the take. A band's magnitude is its power
  mean and its phase that of the complex mean, as ac2's columns read; the recording's delay
  is taken out of the band sums and put back at the centre.
- Columns left out of band comparisons, counted in each row: within 2 Hz of a mains line,
  within 1/12 octave of a sweep's ends (1/6 below the top on speaker paths), and on speaker
  paths comb nulls 10 dB below their 1/3-octave mean and columns whose noise alone exceeds
  the pass limit.
- Groups:
  - **magnitude / phase**: band-wise spread after removing the delay difference (1–20 kHz
    electrical, 200 Hz–5 kHz acoustic). ac2's TF is compared only where γ² ≥ 0.99.
  - **level convention**: ac2 shows meas÷ref, so the loopback gain is in its number. REW
    live shows meas re the digital stimulus. The `offset_live_explained` row should be ~0.
  - **delay**: ac2's arrival and its arrival plus the phase slope, against the direct IR
    peak.
  - **group delay**: central difference (as displayed) and the ±1/12-octave fit, against the
    sine pairs.
  - **harmonics**: per frequency the fundamental, reading, floor, margin and kind for each
    source. Also a floor cross-check from raw noise windows (flagged if more than 6 dB apart)
    and the sweep columns whose harmonics land on mains lines.
  - **LF H2**: the H2 excess over the sine truth per sweep variant, and the frequency up to
    which ac2 overstates.
  - **absolute SPL** (speaker): dB SPL per 0 dBFS drive from the sine, ac2 (meas÷ref +
    reference level + sensitivity), and REW (SPL with the cal from ac2's store).
  - **room** (speaker): ac2's `room_metrics` against REW's RT60 export and a numpy Schroeder
    integration of REW's IR.
  - **ambient SPL**: Leq Z/A/C from ac2, REW and numpy; third-octave levels.

## Baselines and compare

`baselines/<rig>/<stage>-<level>dbfs.json` (`ambient.json` for the silent stage) hold a
reviewed run's verdict and value for every check, its limits, and where it came from (run,
date, ac2 build, REW version, flags, suite commit). Tables, plots and audio stay in the run.

```
python -m crosscheck compare runs/<UTC time>                 # against baselines/, every stage
python -m crosscheck compare runs/<UTC time> --baseline f.json
python -m crosscheck baseline runs/<UTC time> [--stage xone]  # write / update from a run
```

- **Matching**: a check's key is its id with the tone frequency replaced by the one the sine
  plan asked for (`xone.sine_mag.REW offline@50Hz`, whether 50 Hz played at 47.8 or
  51.275 Hz). Without the plan, a tone pairs with the nearest one of the same check within
  6 %. A stage is compared only with the baseline of the same stage and level.
- **compare** lists, per stage, the build pair, status changes (worse / better), judged
  values that moved more than max(unit step, half the pass limit) (`[compare]` in
  `tolerances.toml`), INFO / INCONCLUSIVE values that moved (context only), and checks new or
  missing. It writes `report/compare.md` and exits 1 when a status got worse or a judged value
  moved, so it can gate.
- **Updating a baseline**: after a run on a new build has been reviewed and its differences
  are understood, run `baseline` on it (it prints what changed and refuses a run with FAILs
  unless `--force`, which is for a FAIL that is a known, documented gap) and commit the
  files with the ac2 build hash in the message.

## Fixtures (2026-10-07)

`python -m crosscheck analyse /work/ac2-scratch/crosscheck-fixtures` loads the hand run:
Xone path only, with the documented sine truth in `reference/`. The truth was taken at
−27 dBFS. The hand scripts played amplitude 10^(L/20)·√2, so their "−30 dBFS" is −27 in the
convention used here, 3 dB hotter than the −30 dBFS sweeps it is compared with.
`tests/test_fixtures_smoke.py` checks that the documented numbers come out:
- the GD fit is within 10 % from 16 Hz up;
- REW live's delay matches the direct value;
- ac2's whole-sample arrival is reported;
- the 10 Hz sweeps overstate LF H2.

## Known gaps

- **REW live measurements need Pro**, so only the offline import runs. The fixtures'
  "REW live" set came from a GUI session. REW's sweep length is the file's: make another
  file for another length, or use `[stages.rew].repeats`.
- **Not exercised on the rig yet:** REW's SPL-meter `levels` and `rta/captured-data`
  formats, `Generate RT60` with the `/rt60` export, the RTA mode and averaging names, and
  REW's absolute FR convention for an imported response (the `REW (SPL…)` rows assume FR =
  gain + drive level). Check these rows on the first run.
- **ac2's RTA is captured as one frame:** `meas new rta` has no averaging option, so its
  third-octave row is INCONCLUSIVE (a snapshot against a 60 s average).
- **Speaker paths at −50 dBFS** leave most of 20–1000 Hz below the noise limit: those band
  rows are INCONCLUSIVE, and room clarity (C50/C80) is INCONCLUSIVE when ac2's decay meets
  the noise before the boundary.
- **Emission was not exercised here.** On the dev host the run side was exercised only
  against ac2d's fake backend and a JACK dummy server, and the systemd drop-in path was not
  exercised at all (it restarts the rig's ac2d).
