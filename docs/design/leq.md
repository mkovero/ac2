# Rolling Leq windows, limits and alarms

Status: implemented (`ac2_core::leq`, the SPL meter job, `spl.log_get`, `spl.log_new`,
`spl.history_get`, the SPL pane's Leq view, `ac2 spl leq`). Answers PLAN.md §3.6 "Rolling
Leq windows, limits and alarms" and the log half of "Continuous crash-safe logging,
export".

## What the operator gets

Each SPL meter carries a list of rolling windows — by default LAeq over 1, 5, 10, 30 and
60 min, no limits. A window may have a limit (dB SPL) and a warn margin (default 3 dB).
Every second the daemon publishes, per window: the Leq, how much of the window has elapsed
and how much of it was measured, its state (ok / near / over; a window still filling is
judged on its energy budget, *Judging a filling window*), and the **headroom**: the
highest steady level for the next minute that keeps the window at or below its limit. The
app shows each window amber when near, red when over, back to normal when it recovers — as
columns filling like meter bars or as tiles, with an optional history strip;
`ac2 spl leq watch` is the terminal equivalent.

## One-second blocks

The meter job integrates the mic-curve-corrected input through A, C and Z weighting into
**one-second blocks** aligned to the job's first captured sample:

    e_w = Σ y_w² / fs   (FS²·s, per weighting w)        m = measured samples / fs   (s)

A capture discontinuity (lost samples: a jump in the block sample index) advances the
second grid without adding energy, so the seconds it touches have `m < 1`. A gap is never
silence: it adds neither energy nor measured time.

Every block becomes a **log row**: wall time of its start, `m`, LAeq,1s, LCeq,1s, LZeq,1s
(dBFS, `10·lg(2·e/m)`, decision 4a), and the sensitivity in force (dB SPL of 0 dBFS, if
calibrated). The log is the record: it lives with the meter in the daemon (last 48 h), is
saved with the session and the autosave (one CSV per meter, which the autosave appends to), and is
exported with `spl.log_get` (`ac2 spl leq export`). Frozen or reset meters keep logging:
freeze and reset are display operations, a compliance record is not.

## Windows

A window of N seconds over the newest N slots (a slot per second of time, measured or not):

    Leq_N = 10·lg(2 · Σ e_i / Σ m_i)  dBFS,   + sensitivity → dB SPL
    elapsed = min(N, seconds since the first logged second),  measured = Σ m_i

While filling (`elapsed < N`) the Leq is over the elapsed time. A window with `measured <
elapsed` is flagged **incomplete**; its Leq is over the measured time (IEC 61672-1
time-averaged level of what was measured, never extrapolated). Sums are f64 with Neumaier
compensation, running (add the new slot, subtract the one leaving) and recomputed exactly
from the ring every N slots, so float drift cannot accumulate; tests compare against brute
force sums over random sequences with gaps.

The windows survive the job: stopping and starting the meter, a device reopen, a change to
the windows or a daemon restart (autosave) rebuild them from the log by wall time, with the
time in between as gap.

## Run clock and total

Besides its windows, the log as a whole is a figure the operator asks for ("how long have we
been going, and what is the show's level so far?"). Every `leq` frame carries the log's
**run**, computed by the daemon from the rows it holds:

    started_at = start of the oldest row        until = start of the newest row + 1 s
    LAeq,total = 10·lg(2 · Σ e_i / Σ m_i)        (LCeq, LZeq the same)
    gaps = Σ (1 − m_i) + whole seconds without a row between rows

Exact, as the windows: the sums are compensated (`ac2_core::leq::LogTotal`), kept as rows
come and go and recomputed from the rows once an hour of trimming, so subtracting the rows
the retention drops leaves no residue (tests: brute force over logs with pauses, lost and
partial seconds, and over a trimmed 48 h log). The total is in the frame's unit with the
sensitivity in force, as the windows are; gaps (the meter stopped, the daemon down, lost
samples) are never silence. Because `started_at` comes from the log, the clock carries on
across app restarts, meter restarts and daemon restarts (the log is in the autosave); the
time the daemon was down counts as gap. A log at its 48 h retention is **trimmed**: the
clock and the total then cover the 48 h kept, and the caption says "last 48 h".

The caption: `running 2:14:05 since 19:02 · LAeq total 97.8 · offline 12 s` — the clock always
in hours (it is never a time of day), the start in local time (with the date when not the
newest second's day), LAeq always and LCeq / LZeq when a window uses that weighting, gaps
only when there are some (≥ 1 s). Large (4 % of the pane's height, up to twice the caption
type) between the meter and the calibration so it reads from a distance in the stage view;
narrower panes get `2:14:05 since 19:02 · total 97.8 · offline 12 s`, then `2:14:05 · total
97.8`, and when even that does not fit beside the meter it moves to a row of its own,
shortened down to the clock. Tested at 320–1920 px in columns and tiles: no overlap.

## A new log

`spl.log_new` ends the meter's log and starts an empty one: the windows, their states, the
alarms, the run clock and the total start over, the windows, limits and horizon are kept.
The ended log is not thrown away at once: the daemon keeps it as the meter's **previous** log
(`spl.log_get` with `log: previous`) until the next new log, the meter's deletion or a
daemon restart — only the current log is saved with the session and the autosave. That is
the minimal honest form: `ac2 spl leq new --yes --export FILE` writes the ended log after
the reset, whole, with no second lost between an export and the reset; `ac2 spl leq export
--previous` gets it later. The meter's job sees the new log at its next second (each log has
an epoch) and starts its windows over; judgement reports from before the reset are dropped.
The app asks first (Shift+R in the SPL pane, "Start a new SPL log…"), naming the run that
ends and what starts over; the CLI wants `--yes`.

## Limits

Judged only when the meter is calibrated (values in dB SPL; a calibration from another mic
or input counts, and the tile says so; an electrical calibration counts too, and the caption
names it with its uncertainty, `electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB`,
`q7-calibration.md` §11). Uncalibrated meters show dBFS values, "not
calibrated", and no state. With limit `L` and margin `μ`, at the displayed 0.1 dB
resolution (so the colour never disagrees with the number), a full window is

    over  ⇔ round₁(Leq) > L        near ⇔ L − μ < round₁(Leq) ≤ L        else ok

and a window still filling is judged on its budget (next section).

## Judging a filling window

A limit applies to a full window: every rule in *Presets* defines it over N minutes ("à
aucun moment … sur 15 minutes", "gemeten over 15 minuten"; Brussels' 1 s sliding windows),
and none says how to judge the first N minutes of a measurement, when the window holds less
than N minutes. Judged like a full one, the Leq over the elapsed time puts every window
over its limit at once: a new log at 70 dB with 60 dB limits on 1 to 60 min windows turns
all of them red in the first second, though the 60 min window has an hour to go and could
still end under its limit. ac2 judges a filling window on what it can still do:

    budget  B = P · (M + r)        r = N − elapsed (seconds left to fill),  P = 10^(L/10)
    least   = 10·lg(E / (M + r))   the Leq the window ends at if the rest is silent

with `E` and `M` the energy and measured time so far. `B` is the energy a window may hold
and still end at the limit; `least` reaches the limit exactly when `E` reaches `B`.

    over       ⇔ round₁(least) > L                 (E has spent the budget: a certainty)
    on course  ⇔ not over and round₁(Leq) > L      (near, flagged ON_COURSE)
    near       ⇔ L − μ < round₁(Leq) ≤ L
    else ok

- **Over only when certain.** An alarm, a red column and a log entry say the window *will*
  end over its limit whatever is played from now on. A filling window that is over stays
  over until it is full (E only grows while nothing leaves it), then recovers by the
  rolling rule.
- **Amber = on course.** The Leq so far is the level the full window ends at if the rest
  goes on at the same mean power (`(E + E/M · r) / (M + r) = E/M`), so above the limit it is
  on course to go over; within the margin it is near, as a full window. The time until the
  budget is spent at that pace, `(B − E) / (E/M)`, goes with it: "ON COURSE — over in 12
  min". It is shorter than `r` whenever the Leq so far is above the limit.
- **Once full both rules are one**: `r = 0`, `least` is the Leq, and the rolling rule
  applies unchanged.
- **Gaps** (the meter stopped, lost samples) neither spend nor earn budget: like the Leq,
  which is over the measured time, the budget counts the measured seconds and the ones
  still to come (assumed measured). A window with a gap thus ends with the same judgement
  under both rules, and stays flagged incomplete.
- **Headroom while filling.** With at least the horizon left to fill, nothing leaves the
  window before it is full, so the headroom is the steady level that, held until it is
  full, spends exactly what is left of the budget: `x = (B − E) / r` (the CLI: "until full: stay ≤ 98.2
  dB"); `x ≤ 0` means over (cannot recover; the time to recover at the limit is then at
  least `r`). With less than the horizon left, the rolling headroom below applies.

The user's case: 70 dB against 60 dB limits on a fresh log is ten times the limit's power,
so a window spends its budget in a tenth of its length. The 1 min window is on course from
its first second and over after 6 s (shown over from 7 s, when `least` reads above 60.0 at
0.1 dB), the 60 min one on course for 6 minutes and over after about 6 min 5 s; the 5, 10
and 30 min windows in between. Tests: `ac2_core::leq` (this case second by second, quiet
then loud, gaps, full windows identical to the rolling rule), the daemon's alarm timing
(`crates/ac2d/tests/leq.rs`), the scene's texts and bar, `ac2 spl leq watch --json`, and the
app from an empty daemon.

This is ac2's presentation choice, not a regulation's: the regulations judge full windows,
and ac2 reports OVER before a window is full only when the full window cannot end under
the limit.

**Headroom** of a full window (or one with less than the horizon left to fill; a window
filling for longer is in the section above) over a horizon of h seconds (default 60):
after h more seconds the window holds
the newest `K = N − h` slots of today plus h new ones. With `E_K`, `M_K` the energy and
measured time of those K slots and `P = 10^(L/10)` (as a mean square), the steady level x
that lands exactly on the limit solves `(E_K + h·x) / (M_K + h) = P`:

    x = (P·(M_K + h) − E_K) / h        (N ≤ h: x = P)

reported floored to 0.1 dB. When `x ≤ 0` the window **cannot recover within h** whatever
happens; then the daemon reports the time to recover when playing at the limit: the
smallest t ≥ h with `E_(N−t) ≤ P·M_(N−t)` (the part still inside the window averages at or
below the limit), found by one pass over the ring. The display words it **"cooling down in
7 min 30 s"**: the time until the window is back under its limit if the level stays at the
limit (narrow "cooling down in 7:30", "cooling 7:30", "7:30"); the headroom reads "next 1
min: stay ≤ 101.5 dB" in the CLI and just "stay ≤ 101.5 dB" in the app (narrow: "stay ≤
101.5", "≤ 101.5"), where the level to stay under is what is acted on and a second line
for what it holds for would compete with it.

State changes go into the meter's `spl_log` entity: each window's state with the time it
began, and an alarm list (window, over / recovered, time, Leq, limit; newest 100). The
daemon logs each over and recovery. Clients toast them. There is no hysteresis beyond the
0.1 dB resolution: a window that hovers on its limit reports each crossing.

## Where it shows

- App: **G** steps the SPL pane meter → windows → meter + windows (the meter's number over
  the windows, the default; `ac2_scene::meter_leq`); **B** lays the
  windows out as columns or tiles, **Shift+B** shows the history strip (rebuilt from the
  log, *The history strip* below), both remembered in `ui.toml`; **W** steps split → the pane
  alone → full screen (the stage view); **F11** puts the window full screen in any layout. **Shift+L** opens the windows
  dialog (lengths and weightings picked, limits typed, a preset row, the horizon). Over /
  recovered alarms are toasts. See *Display* below.
- CLI: `ac2 spl leq watch` (block digits on a terminal, `--json` a line a second, with the
  run), `ac2 spl leq set` (`--windows`, `--preset`, `--limit 30min=99db`, `--warn`,
  `--horizon`), `ac2 spl leq export` (the CSV; `--previous` the ended log), `ac2 spl leq
  new` (`--yes`, `--export FILE`).

What is left: `docs/design/backlog.md` (peak limits, position correction, alarm
hysteresis).

## The history strip

Each window's Leq over time, against its limit, red where the window was over: one point
a second, the newest 4 h kept, the strip showing twice the longest window (2 min … 2 h).
The points are what the `leq` frames carried, so a client that was not connected — the app
restarted, another machine, a reconnect, a daemon restart — gets them from the daemon
(`spl.history_get`) instead of starting empty. The daemon computes them: the app computes
no measurement values (it does not link the DSP), and the daemon holds both the log and the
code that made the frames.

- **When**: the app asks whenever it first sees a meter (connected, resynced, a meter
  created), when the meter's windows change (new windows get their past too) and when a new
  log starts — from this app or any other client: the log empties (`spl_log.started_at`
  back to none) or its rows are numbered from 0 again (the frame's `logged` falls). A new
  log clears the history first. An answer to an earlier request is dropped.
- **What**: the newest 4 h (`SplHistory::MAX_SECONDS`) of the *current* log, one point per
  logged second: its end, and per window the Leq (f32 in the meter's unit, as the frame
  carries it) and whether it was over. The daemon reads 4 h plus the longest window of rows
  (at most 5 h of a log that keeps 48): the longest window before the first point is what
  that point is computed over; the rest of a 48 h log would only make points the strip
  drops.
- **How**: `ac2_core::leq::LogReplay` replays the rows as the job computed them — a second
  without a row while running is a gap pushed into the windows; a stretch without rows is
  where the meter was stopped or the daemon was down, and the job that started after it
  refilled its windows from the rows in its span (`RollingLeq::refill`, which the job
  itself calls), so the windows count as elapsed from the oldest of those; a stretch longer
  than the longest window empties them. Each second is judged with `judge_window` against
  the window's limit and the row's sensitivity (a filling window on its budget), as the job
  judged it. Begun part way through the log, the seconds before the longest window has
  filled with replayed rows are left out: they lack what came before. Only the seconds
  after the last change of unit (a calibration) are given, as the history starts over at
  one live. The rows are copied out of the log under its lock; the replay runs outside it.
- **Joining live**: the rebuilt points replace the series up to the newest of them; frames
  received after that second continue it. A frame within half a second of the newest point
  is the same second (a point is stamped at the end of its second, a frame a few
  milliseconds later), so no second appears twice. A history in another unit than the
  frames received since (calibrated meanwhile) is dropped.

The only difference from the frames received live: a stretch of seconds without rows
while the job ran (whole seconds lost to capture gaps) is taken as a restart, which can
make a window count as filling a few seconds longer than the job did when such a stretch
falls exactly at the start of its span. Tests: the replay against the job second by second
over a log with lost seconds, short and long pauses (`ac2_core::leq`); the history bit for
bit against frames built as the job builds them, from the log's start and part way, and
across a change of unit (`ac2d` `leq_history`); a log longer than a page, paged and its
4 h history (`crates/ac2d/tests/leq.rs`); the history joining live frames
(`ac2_scene::leq`); the app's reducer (restart, a new log from elsewhere, changed windows,
reconnect); and from an empty daemon, a restarted app shows the seconds before it started
within 0.01 dB of what the first app received, and a new log from one app clears another's.

## Display

The columns are the default: the view is for whoever reads it from a distance (stage,
FOH, performers), and a row of bars filling towards a line reads at a glance where a grid of
figures does not. All decisions are `ac2_scene::leq` (headless, tested); the app only draws.

- **Order**: one full-height column per window, by length, shortest left (equal lengths: A,
  C, Z), whatever order the configuration lists them in.
- **Scale**: one for all columns, so bars compare. With judged limits: from 30 dB below the
  lowest limit to 6 dB above the highest (`BELOW_LIMIT_DB`, `ABOVE_LIMIT_DB`) — anchored to
  the limit, so the line sits at the same height show after show, and windows with different
  limits (93 and 100 dB) share a scale that covers both. Without a judged limit (no limits,
  or uncalibrated dBFS): 40 dB whose top is a multiple of 10 dB at least 5 dB above the
  loudest window; it is kept while the loudest window stays between 20 and 2 dB under its
  top, so it steps only when a level nears the top or has fallen well below it (up at 98,
  back down below 90 on a 100 dB top). The memory lives in `LeqHistory` with the frames.
  Levels off the scale fill the track or leave it empty; the value on top is always exact.
- **Colour**: judged only (as the tiles). Over: the bar in the fault colour and the whole
  column tinted towards it; near: an amber bar and a lighter tint; ok: the theme's
  `level_ok` bar; not judged: a neutral bar. A window still filling draws its bar at
  `least` (the Leq it ends at if the rest is silent, *Judging a filling window*): the budget
  spent, rising to the limit line as it runs out and reaching it when going over becomes
  certain; its value stays the Leq so far, and its progress is written above its name ("so
  far · 12:30 / 30:00"). An ok one has its bar part way between the track and its colour, a
  level visibly not a whole window. On course, the state line says so with the time until
  the budget is spent ("ON COURSE — over in 12 min", then "over in 12 min", "ON COURSE" as
  the column narrows), and the headroom holds until the window is full (still worded
  "stay ≤ 98.2 dB"). Tiles show the same texts. The limit is a line across the whole column.
- **Text**: what a performer acts on is large — the state (`OVER`, `NEAR`, `OK`, "over in
  47 s" on course) and the instruction ("stay ≤ 101.5 dB" alone, not what it holds for;
  "cooling down in 7 min 30 s" when it cannot recover within the horizon) — each in the
  longest wording that fits every column saying the same kind of thing, shrinking to half
  its size before a shorter one is used, so neighbours read alike ("stay ≤ 102.0" beside
  "stay ≤ 85.3"); the limit small under them (dim unless the column is alarmed), as the line
  across the bar shows it; "not calibrated" in its place when nothing is judged (nothing
  judged is nothing large). The window's value is secondary: a small figure **held still
  low in the track** — the same place at any level, since a figure riding the bar's top
  moves every second and draws the eye for nothing — smaller than the window's name under
  the column (`NAME_RATIO`), at most 0.8 of the state's size (`VALUE_RATIO`) and well under
  the meter's number above the windows (≤ 0.3 of it, tested). Every window's value is one
  size, a window without a limit too, though its column is then mostly empty: values read as
  one row only when they match. Its colour is judged on what
  is behind it (`Behind`): on the fill (the usual case: the bar is above it) the theme ink
  of highest contrast with the fill, ≥ 4.5:1 (white on red, black on amber or green, never
  red on red); on the track (the level below it) the bar's own colour where that reads
  (≥ 3:1), plain text otherwise, dim before anything was measured; across the fill's edge
  the ink that reads on both. The limit line is 30 dB above the scale's bottom on a judged
  scale, far above the figure; on a track too short for that the figure goes just above
  the line. On its baseline, smaller, the window's own unit and weighting (`dB(A)`,
  `dB(C)`, `dBFS (A)` uncalibrated; tiles the same), never dropped, since the SPL meter
  shown above the windows may use another weighting. Rows are left out when the bar would
  get too short (the limit first, the instruction last); the name at the bottom, shortened
  uniformly when narrow (`LAeq 30 min` → `30 min` → `30m`, the caption then names the
  weighting; mixed weightings keep their letter). Tested: 1–8 windows, 320–3840 px, no text
  overlaps, every value inside its track, low, at the same place from the bottom of the
  scale to over its top, clear of the limit line and legible on what is behind it.
- **Tiles** (B) keep every figure written out in a grid: the name and state on top, in the
  middle the instruction large, the value smaller than the name at a fixed place low in the
  body (one size and place in every tile of the grid, with an instruction or not), the
  course, limit and progress below; over tiles fill red, near ones amber; the **history strip** (Shift+B) goes under
  either. Defaults: columns, no strip.
- **Stage view**: full screen with the SPL pane maximised on its windows draws only the
  scene — columns and the caption (meter, unit, run, calibration) — without the app's top bar,
  measurement list or pane title. While a stimulus is armed or playing, or a sweep runs, the
  top bar is shown anyway: what drives the speakers is never hidden.

## Presets (informational, not legal advice)

A preset **replaces** the meter's windows with exactly the rule's: its windows with their
weightings and limits, plus any window the rule wants shown without a limit (Flanders 100
dB shows LAeq 15 min), shortest first (equal lengths A, C, Z). Windows and limits the rule
does not state go: left in place they would read as part of it. The windows change in
place (`meas.update`): the log carries on and the new windows are rebuilt from it, as for
any change of windows. Windows can be added afterwards (Insert in the dialog, `--windows`
with `--preset`). In the app, ←/→ on the preset row shows each preset's windows in the
table, and back at "none" the windows as they were before the row was first changed
return; an edit to a window keeps what the preset set. A note under the row says the
preset replaces the windows. `ac2 spl leq set --preset` replaces them likewise; several
`--preset` give the windows of all of them, a window two share with the lower of their
limits (both rules met) and a limit winning over a window only shown — at most five
distinct windows across all presets, within the eight a meter may have. `--windows` with
`--preset` adds windows (without limits) to the preset's.

Every preset is a starting point for the operator, who owns the rest of the rule:
ac2 judges only the Leq windows below, with no position correction, and is not a
type-approved instrument (see the last section). Retrieved 2026-10-03.

| preset (`--preset`) | windows | source |
|---|---|---|
| DIN 15905-5 (`din15905`) | LAeq 30 min ≤ 99 dB | DIN 15905-5:2007, loudest audience position |
| Swiss V-NISSG 93 / 96 / 100 (`swiss93` …) | LAeq 60 min ≤ 93 / 96 / 100 dB | V-NISSG (SR 814.711), by event category |
| WHO safe listening (`who`) | LAeq 15 min ≤ 100 dB | WHO Global standard for safe listening venues and events (2022) |
| France R1336-1 (`france`) | LAeq 15 min ≤ 102 dB **and** LCeq 15 min ≤ 118 dB | [1] art. R1336-1 II 1° |
| France R1336-1, children up to 6 (`france-children`) | LAeq 15 min ≤ 94 dB **and** LCeq 15 min ≤ 104 dB | [1] art. R1336-1 II 1°, second sentence |
| Flanders VLAREM 85 dB (`flanders-85`) | LAeq 15 min ≤ 85 dB | [2] art. 6.7.3 § 1 |
| Flanders VLAREM 95 dB (`flanders-95`) | LAeq 15 min ≤ 95 dB | [2] art. 5.32.2.2bis § 1, 1° (and art. 5.32.3.10 § 1) |
| Flanders VLAREM 100 dB (`flanders-100`) | LAeq 60 min ≤ 100 dB; LAeq 15 min shown | [2] art. 5.32.2.2bis § 2, 1° and 3° |
| Brussels 85 dB (`brussels-85`) | LAeq 15 min ≤ 85 dB | [3] art. 3 § 1 |
| Brussels 95 dB (`brussels-95`) | LAeq 15 min ≤ 95 dB **and** LCeq 15 min ≤ 110 dB | [3] art. 4 § 1 |
| Brussels 100 dB (`brussels-100`) | LAeq 60 min ≤ 100 dB **and** LCeq 60 min ≤ 115 dB | [3] art. 5 § 1 |
| NL covenant 103 dB (`nl-covenant`) | LAeq 15 min ≤ 103 dB | [4] art. 3.1.2 |
| NL covenant, ages 16–17 (`nl-covenant-16-17`) | LAeq 15 min ≤ 100 dB | [4] art. 3.1.3 c |
| NL covenant, ages 14–15 (`nl-covenant-14-15`) | LAeq 15 min ≤ 96 dB | [4] art. 3.1.3 b |
| NL covenant, ages up to 13 (`nl-covenant-13`) | LAeq 15 min ≤ 91 dB | [4] art. 3.1.3 a |

ac2's windows slide in one-second steps, which is how Brussels defines its windows
(art. 1 § 1, 4°–7°); the other texts say "over 15 minutes" without fixing the step.

### Sources and what ac2 does not check

**[1] France** — Code de la santé publique, art. R1336-1, as modified by décret
n° 2017-1244 du 7 août 2017 (art. 1), version in force since 10 August 2017.
<https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000035425898>. Applies to places
open to or receiving the public, closed or open, that diffuse amplified sound above the
equal-energy rule of 80 dB(A) over 8 h. II 1°: "Ne dépasser, à aucun moment et en aucun
endroit accessible au public, les niveaux de pression acoustique continus équivalents 102
décibels pondérés A sur 15 minutes et 118 décibels pondérés C sur 15 minutes"; for
activities "spécifiquement destinées aux enfants jusqu'à l'âge de six ans révolus", 94 dB(A)
and 104 dB(C) over 15 minutes. Not checked by ac2: the limit holds at any place accessible
to the public (ac2 measures where its mic is); continuous recording of the A and C levels
and keeping the recordings (2°, venues over 300 people and all discothèques); showing the
levels continuously near the sound control position (3°); informing the public of the
risks (4°); free hearing protection (5°); rest zones or periods at or below the
80 dB(A) / 8 h rule (6°); the étude de l'impact des nuisances sonores; the exemptions
(cinemas, art schools; 2°–6° only for places diffusing amplified sound regularly, except
festivals). Measuring and recording details are in the arrêté du 17 avril 2023 taken under
art. R1336-1 to R1336-16 (not used for any figure here).

**[2] Flanders** — VLAREM II (Besluit van de Vlaamse Regering van 1 juni 1995 houdende
algemene en sectorale bepalingen inzake milieuhygiëne), consolidated text on Codex
Vlaanderen: <https://codex.vlaanderen.be/Portals/Codex/documenten/1003794.html>.
Art. 5.32.2.2bis (version from 1 October 2019), art. 5.32.3.10 (1 October 2019), art. 6.7.3
(4 October 2014).
- 85 dB (art. 6.7.3 § 1): music in tents, in the open air and in public places other than
  those under rubriek 32.1 / 32.2: LAeq,15min ≤ 85 dB(A) anywhere the public normally is
  (§ 2); deemed met when LAmax,slow ≤ 92 dB(A). Louder only with the municipality's
  permission (§ 3: at most LAeq,60min 100 dB(A), a special occasion, and for halls at most 12
  occasions a year, 2 a month, 24 calendar days), then under art. 5.32.2.2bis.
- 95 dB (art. 5.32.2.2bis § 1, also 5.32.3.10 § 1 for rubriek 32.2.2°): music activities over
  85 and up to 95 dB(A) LAeq,15min: LAeq,15min ≤ 95 dB(A), deemed met when LAmax,slow
  ≤ 102 dB(A), music and ambient sound both counted.
- 100 dB (art. 5.32.2.2bis § 2): music activities over 95 dB(A) LAeq,15min: LAeq,60min ≤ 100
  dB(A), deemed met when LAeq,15min ≤ 102 dB(A); LAeq,60min and LAeq,15min measured
  continuously, LAeq,15min continuously visible, LAeq,60min registered. § 3: over 100 dB(A)
  LAeq,60min is forbidden.

Not checked by ac2: the measuring position (meetplaats, bijlage 5.32.2.2bis art. 1) and the
meter requirements (bijlage 5.32.2.2bis art. 2); the "deemed met" LAmax,slow alternatives;
keeping the registered LAeq,60min for at least a month; posting the maximum level at the
entrance and the mixing desk; acting at once on an overshoot; a sound limiter in place of
measuring; free single-use hearing protection and a geluidsplan by an accredited expert
(100 dB); the municipal permission and its count limits; which category an activity falls
in (venue class, rubriek 32).

**[3] Brussels-Capital** — Arrêté du Gouvernement de la Région de Bruxelles-Capitale du 26
janvier 2017 fixant les conditions de diffusion du son amplifié dans les établissements
ouverts au public, Moniteur belge 21 February 2017 (no. 2017010520, p. 27008), in force
21 February 2018, erratum MB 31 January 2019.
<https://www.ejustice.just.fgov.be/cgi_loi/change_lg.pl?language=fr&la=F&cn=2017012632&table_name=loi>;
summary by Bruxelles Environnement:
<https://environnement.brussels/thematiques/bruit/son-amplifie-electroniquement>.
Art. 3: LAeq,15min ≤ 85 dB(A). Art. 4 (derogation): LAeq,15min ≤ 95 dB(A) and LCeq,15min
≤ 110 dB(C). Art. 5 (derogation): LAeq,60min ≤ 100 dB(A) and LCeq,60min ≤ 115 dB(C).
Not checked by ac2: the pictogram informing the public (art. 4, 5); a level display per
room or stage, with recording for establishments under rubrique 135C (after midnight) and
always for art. 5; the display microphone between the public and the main loudspeakers,
1.20–5 m high, calibrated yearly, with a correction when it cannot be there (art. 4 § 1
b); earplugs, a rest zone (≤ 85 dB(A) LAeq,15min, about 10 % of the floor) and a trained
reference person (art. 5); the declaration or permit (art. 7); control measurements
anywhere the public normally is, 1.20–1.50 m high (art. 6). The annex form pairs the
levels differently from art. 3–5; the presets follow the articles.

**[4] Netherlands** — Vierde convenant preventie gehoorschade versterkte muziek, signed by
the Ministry of VWS and sector organisations, term to 6 December 2027 (art. 6), published
in Staatscourant 2024, 3787 (8 February 2024).
<https://zoek.officielebekendmakingen.nl/stcrt-2024-3787.html>. A voluntary covenant, not
statute: it binds its parties' members, and any party may leave with three months'
notice. Art. 3.1.2: "maximaal Leq=103 dB(A), gemeten over 15 minuten"; art. 3.1.3: up to
13 years 91 dB(A), 14 and 15 years 96 dB(A), 16 and 17 years 100 dB(A), each over 15
minutes. Not checked by ac2: the measuring protocol (Meetprotocol convenant geluid
Nederland 2019, bijlage 2: 2 m above the floor in the middle of the audience area, a
correction when measured elsewhere, a meter to IEC 61672-1 calibrated at least every two
years); sharing the results with the registering party (art. 3.2.6); facilitating hearing
protection from 88 dB(A) for minors and 92.5 dB(A) for adults (art. 3.3); visitor
information (art. 3.4); the lower levels in the parties' own appendices (e.g. cinemas).

**Not added**
- Wallonia: no preset. The arrêté du Gouvernement wallon du 13 décembre 2018 fixant les
  conditions de diffusion du son amplifié électroniquement dans les établissements ouverts
  au public has not entered into force (Service public de Wallonie,
  <https://environnement.wallonie.be/home/gestion-environnementale/risques-continus-et-pollutions/nuisances-sonores/sources-specifiques/musique-amplifiee-electroniquement.html>);
  the same page says the arrêté royal du 24 février 1977 applies to music, whose art. 2
  (original text, <https://wallex.wallonie.be/eli/arrete/1977/02/24/1977022408/1977/05/01>)
  sets a maximum level of 90 dB(A) rather than an equivalent level over a window, which a
  rolling Leq cannot stand for.
- Netherlands, statute: no statutory audience limit was found; the covenant is the only
  figure.

## What is and isn't claimed

- Leq is the IEC 61672-1 time-averaged level of the A/C/Z-weighted signal; the weighting
  filters meet class 1 tolerances at 44.1/48/96 kHz (`ac2_core::weighting` tests). The
  instrument as a whole is not a type-approved sound level meter: the microphone, its
  calibration and the interface are the operator's.
- Windows move in one-second steps. No measuring-position correction (FOH → audience) is
  applied; a regulation that wants one needs it added to the limit by hand.
