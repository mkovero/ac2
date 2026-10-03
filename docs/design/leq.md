# Rolling Leq windows, limits and alarms

Status: implemented (`ac2_core::leq`, the SPL meter job, `spl.log_get`, the SPL pane's Leq
view, `ac2 spl leq`). Answers PLAN.md §3.6 "Rolling Leq windows, limits and alarms" and the
log half of "Continuous crash-safe logging, export".

## What the operator gets

Each SPL meter carries a list of rolling windows — by default LAeq over 1, 5, 10, 30 and
60 min, no limits. A window may have a limit (dB SPL) and a warn margin (default 3 dB).
Every second the daemon publishes, per window: the Leq, how much of the window has elapsed
and how much of it was measured, its state (ok / near / over), and the **headroom**: the
highest steady level for the next minute that keeps the window at or below its limit. The
app shows one tile per window, amber when near, red when over, back to normal when it
recovers, with a history strip below; `ac2 spl leq watch` is the terminal equivalent.

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
saved with the session and the autosave (session format 6, one CSV per meter), and is
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

## Limits

Judged only when the meter is calibrated (values in dB SPL; a calibration from another mic
or input counts, and the tile says so). Uncalibrated meters show dBFS values, "not
calibrated", and no state. With limit `L` and margin `μ`, at the displayed 0.1 dB
resolution (so the colour never disagrees with the number):

    over  ⇔ round₁(Leq) > L        near ⇔ L − μ < round₁(Leq) ≤ L        else ok

**Headroom** over a horizon of h seconds (default 60): after h more seconds the window holds
the newest `K = N − h` slots of today plus h new ones. With `E_K`, `M_K` the energy and
measured time of those K slots and `P = 10^(L/10)` (as a mean square), the steady level x
that lands exactly on the limit solves `(E_K + h·x) / (M_K + h) = P`:

    x = (P·(M_K + h) − E_K) / h        (N ≤ h: x = P)

reported floored to 0.1 dB. When `x ≤ 0` the window **cannot recover within h** whatever
happens; then the daemon reports the time to recover when playing at the limit: the
smallest t ≥ h with `E_(N−t) ≤ P·M_(N−t)` (the part still inside the window averages at or
below the limit), found by one pass over the ring.

State changes go into the meter's `spl_log` entity: each window's state with the time it
began, and an alarm list (window, over / recovered, time, Leq, limit; newest 100). The
daemon logs each over and recovery. Clients toast them. There is no hysteresis beyond the
0.1 dB resolution: a window that hovers on its limit reports each crossing.

## Where it shows

- App: **G** switches the SPL pane between the meter and its tiles (amber near, red over,
  with the headroom; **W** maximises the pane, **F11** goes full screen) and a history strip
  of each window against its limit, from the frames received since the app connected.
  **Shift+L** opens the windows dialog (lengths and weightings picked, limits typed, a
  preset row, the horizon). Over / recovered alarms are toasts.
- CLI: `ac2 spl leq watch` (block digits on a terminal, `--json` a line a second),
  `ac2 spl leq set` (`--windows`, `--preset`, `--limit 30min=99db`, `--warn`, `--horizon`),
  `ac2 spl leq export` (the CSV).

What is left: `docs/design/backlog.md` (history backfill, peak limits, position
correction, a fresh start of the windows, alarm hysteresis).

## Presets (informational, not legal advice)

| preset | window | limit | source |
|---|---|---|---|
| DIN 15905-5 | LAeq 30 min | 99 dB | DIN 15905-5:2007, loudest audience position |
| Swiss V-NISSG 93 / 96 / 100 | LAeq 60 min | 93 / 96 / 100 dB | V-NISSG (SR 814.711), by event category |
| WHO safe listening | LAeq 15 min | 100 dB | WHO Global standard for safe listening venues and events (2022) |

A preset sets the limit on its window (adding it if missing) and leaves the others. Each
regulation has more to it (LCpeak / LAFmax limits, where to measure, position corrections,
notification and documentation duties); the operator owns that.

## What is and isn't claimed

- Leq is the IEC 61672-1 time-averaged level of the A/C/Z-weighted signal; the weighting
  filters meet class 1 tolerances at 44.1/48/96 kHz (`ac2_core::weighting` tests). The
  instrument as a whole is not a type-approved sound level meter: the microphone, its
  calibration and the interface are the operator's.
- Windows move in one-second steps. No measuring-position correction (FOH → audience) is
  applied; a regulation that wants one needs it added to the limit by hand.
