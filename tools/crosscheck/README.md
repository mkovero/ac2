# crosscheck: ac2 against REW and steady-sine truth

An on-demand suite, not run in CI. It measures the same paths with ac2, with REW (offline
import of a recording), with steady sines analysed in numpy, and from the raw recordings
themselves. Then it compares the numbers against the tolerances in `tolerances.toml`. It
reruns the hand comparison of 2026-10-07 (`docs/rigs/pupu.md`, "REW cross-check, electrical")
and adds an ambient SPL check and the speaker path.

```
crosscheck/      the package (python -m crosscheck {preflight,run,analyse})
rigs/pupu.toml   ports, roles, paths, levels, stage settings for pupu
tolerances.toml  PASS / WARN limits for every comparison
reference/       documented truth of 2026-10-07 (used when analysing the fixtures)
rig-run.sh       dev host: copy to the rig, run there, fetch the run, analyse here
tests/           pytest: dsp, safety policy, JACK dummy server, fixtures smoke test
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

The **electrical −30 dBFS** run has the Xone stages at −30 and the Genelec stages still at −50:

```
./rig-run.sh --emit -30dbfs --allow-electrical-level -30dbfs --emit-speaker -50dbfs
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
`runs/` here (git-ignored; set `CROSSCHECK_RUNS` to change that) and writes
`runs/<UTC time>/report/`.

### The Genelec stage is audible: the operator must be present

Out 1 drives the Genelec 1083. At −50 dBFS a 1 kHz sine gives about **63 dB SPL at the mic**:
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
- **The speaker never goes above −50 dBFS**, whatever the config or the other flags say
  (`levels.SPEAKER_HARD_MAX_DBFS`). The Xone stages refuse out 1 (`forbidden_outputs`).
- **Above the rig's −50 needs `--allow-electrical-level`**, capped at −30 by the config and
  in code. That flag installs a runtime systemd drop-in, restarts ac2d with
  `--max-level -30` and reopens the session. This happens only **after** the speaker stage.
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
3. **xone** at `--emit`: out 3 → Xone → in 5; reference out 2 → in 2. With
   `--allow-electrical-level`, the drop-in is installed just before this stage.

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

## Reading the report

`report/report.md` starts with the counts and the failures and warnings. Then come the
calibration mapping, one table of checks per group, the data tables and the plots.
`results.json` holds the same data, machine-readable.

- **PASS / WARN / FAIL** compare `|value|` with (pass, warn) from `tolerances.toml`. Each row
  says what was compared with what.
- **INCONCLUSIVE** means a reading below floor + `margin_db` (10 dB). Such a reading is an upper
  bound "< X", never a value, and the row gives the shortfall. Comparisons that rest on bounds
  never PASS or FAIL. One exception: a value more than the warn limit above a bound is a FAIL.
- **direct** sources are numpy cross-spectra of the same recording an app analysed. A
  difference to them is the app's analysis, not the take.
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
- **ac2's RTA is captured as one frame:** `meas new rta` has no averaging option. Its
  third-octave rows compare a snapshot with a 60 s average, so expect WARN in quiet,
  fluctuating rooms.
- **The ac2 arrival is whole-sample** in the builds of 2026-10-07: about 3.7 µs short on
  the Xone path. The total-in-phase row is the one to judge.
- **The speaker-path analysis has never seen real data.** It ran only on synthetic run
  directories; on the first operator run, check the table shapes before trusting the
  numbers.
- **Emission was not exercised here.** On the dev host the run side was exercised only
  against ac2d's fake backend and a JACK dummy server, and the systemd drop-in path was not
  exercised at all (it restarts the rig's ac2d).
