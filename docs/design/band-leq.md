# Band Leq: 1/3-octave band limits where the neighbours are

Status: stages 1 to 5 implemented (`ac2_core::band_leq`; the daemon's band meter,
`spl.band_transfer`, the `band_leq` frame, the band log; the band view, the dialog rows and
`ac2 spl bands`; band windows configured like the Leq windows).

## What the operator and the artist get

Some rules limit noise per 1/3-octave band at a **receiving place** — a neighbour's
bedroom or living room, a flat across the yard. The case ac2 was built for, the Finnish
decree STM 545/2015 (asumisterveysasetus), limits the low-frequency noise in rooms meant for
sleeping per band, 20 to 200 Hz, as the unweighted average over an hour, and music noise at
night as LAeq,1h 25 dB; its living-room table limits the LAeq only. Other rules, house
agreements or an operator's own targets differ in the bands, the averaging time, the
weighting and the limits, so the band meter is configured like the Leq windows: **band
windows** of any length and weighting, each with its own per-band limits (one set, or night
limits with a day offset) and warn margin, on the **bands the operator keeps**.

Such limits hold where nobody stands with a mic during a show. ac2 therefore measures at FOH
and carries the figures to the place with a transfer measured once at setup: per band, how
much quieter the place is than FOH. A band limit at FOH is the place's limit plus that
attenuation; the same transfer, with A weighting at the band centres, predicts the place's
LAeq. The operator names the place (`flat 4 bedroom`; `receiving room` unless named), and
every line about it says that name. The artist sees each window's bars against their limit
lines, the headroom per band, the worst band of the worst window named, the time to recover
when one is over, and the predicted LAeq at the place against its limit.

## Quantities

The decree's band limits are on the **unweighted** (Z) band level: "pienitaajuisen
sisämelun tunnin keskiäänitaso" per 1/3-octave band, an energy average over an hour. The
SPL meter's mic-curve-corrected input, before any frequency weighting, goes through the
IEC 61260-1 class 1 1/3-octave filter bank of `ac2_core::rta` (multirate: the low bands
run decimated, so twenty bands below 200 Hz cost little), and each band's output is summed
into the same **one-second blocks** as the A/C/Z log (`docs/design/leq.md` *One-second
blocks*):

    e_b = Σ y_b² / fs   (FS²·s, per band b)        m = measured samples / fs

A capture gap advances the second grid without energy or measured time, exactly as for the
A/C/Z seconds; the filters start again from rest after it, since their state belongs to
samples no longer adjacent to the next ones (the 20 Hz band, 4.6 Hz wide, settles within
a few tenths of a second, so only the second after a gap reads slightly low).

**Bands: 1/3 octaves 20 Hz … 10 kHz, 28 of them**, every one integrated and logged
whatever is shown. The Finnish set is the first eleven (20 … 200 Hz). The wider set is
integrated because the predicted LAeq at the place is A-weighted:
music through a building is dominated by the bass, but 250 Hz … 2 kHz still carry
A-weighted energy where the A weighting stops taking 10–30 dB away and the wall's loss has
not yet grown large, and leaving them out would understate the prediction. Above 10 kHz
the A-weighted contribution after a building element is negligible (its loss there is far
larger than the music's level above the place's background), and stopping at 10 kHz
keeps every band below Nyquist at 44.1 kHz with margin (upper edge 11.2 kHz). The count is
fixed: a second is a `[f64; 28]` plus its measured time, never a `Vec`.

**Stored per second** (stage 2's log row): the wall time of its start, `m`, the 28 band
levels (dBFS, `10·lg(2·e/m)`, 0.01 dB), the §13 correction in force, the period it was
judged in (day / night, *Day and night*) and the sensitivity in force. The levels are stored
uncorrected; the correction is a column of its own, so a correction typed later or wrongly
can be reviewed against the raw record.

## Windows and judging per band

A **band window** (`BandWindow`) has a length (1 s … 24 h in whole seconds), a weighting
(Z, A or C; *Weighting a band*), limits per band (`BandLimitSet::Always`, one set, or
`NightDay`, night limits and a day offset; *Day and night*) and a warn margin. A meter has
up to eight, like its Leq windows, and every window covers the same **shown bands**
(`BandLeqConfig.bands`, any of the 28, at least one): only those are drawn, judged and
alarmed. A band of a window is a **rolling window**, one-second steps, built on the same
ring and sums as the A/C/Z windows (`ac2_core::leq::RollingLeq`, generic over
the slot: a band second is just another set of channels). Everything in
`docs/design/leq.md` *Windows*, *Limits*, *Judging a filling window* and the headroom /
recovery rules applies per band unchanged: the Leq over the measured time, a gap never
silence, f64 Neumaier sums recomputed exactly every window length, a filling window judged
on its energy budget (over only when certain, on course when the pace so far ends over),
the headroom as the highest steady level for the horizon, and "cooling down in …" as the
time to recover playing at the limit. Tests compare every band against brute-force sums
over random sequences with gaps.

**The worst band** is named, per window and across them: the most severe judgement (over,
on course, near, ok), then the band furthest above (or least below) its limit. That is the
one the artist acts on — typically one kick or bass note's band — so the stage view leads
with it, named with its window (`63 Hz band LZeq 60 min 3.2 dB over its limit · cooling
down in 6 min 52 s`).

## Weighting a band

A window's weighting is applied per band as a level offset: the IEC 61672-1 analytic
weighting at the band's **exact mid-band frequency** (`1000 · 10^(x/10)` Hz) added to the
unweighted band level (`ac2_core::band_leq::weighting_db`). The log keeps the unweighted
levels, so every weighting is computed from the same record and a window's weighting can be
changed afterwards.

Weighting each band at one frequency is not the same as filtering by the weighting first:
the weighting varies across a band. For a band filled evenly the mid-band offset reads low
by (ideal band edges, energy averaged across it):

| band | A, pink | A, white | C, either |
|---|---|---|---|
| 20 Hz | 0.32 dB | 0.43 dB | ≤ 0.06 dB |
| 63 Hz | 0.13 dB | 0.20 dB | ≤ 0.01 dB |
| 200 Hz | 0.04 dB | 0.09 dB | ≤ 0.01 dB |
| 1 kHz and above | ≤ 0.01 dB | ≤ 0.03 dB | ≤ 0.03 dB |

A single tone at a band's edge is weighted by the mid-band figure, not its own: for A that
is off by up to ±3 dB at 20 Hz, ±2 dB at 63 Hz, ±1.2 dB at 200 Hz and ±0.4 dB at 1 kHz;
for Z nothing. A per-band rule written on Z levels (as the Finnish one is) is exact; a
weighted band window is the band's share of a weighted level, close for broadband music and
an approximation for a tone at a band's edge. Filtering the input once per weighting before
the bank would cost a filter bank per weighting in use for a few tenths of a dB on broadband
signals; the offset keeps one bank, one log and any weighting after the fact.

## Day and night

A window with one set of limits (`Always`) is judged by them day and night. A `NightDay`
window has night limits and a day offset; the Finnish preset's is +5 dB. The decree's night is 22:00–07:00 and its day 07:00–22:00, local time; the day limits of
the low-frequency table are 5 dB higher ("Päiväajan (klo 7–22) pienitaajuiselle melulle
sovelletaan 5 dB suurempia arvoja kuin taulukossa 2"). Core has no clock: the caller gives
each second's **period from the local wall time of its start** (`Period::at`, the daemon
converting UTC with the host's time zone rules).

**A window is judged by the night limits when any second it holds was a night second**, by
the day limits otherwise. An hour window straddling 22:00 or 07:00 holds both:

- At 22:00 every rule agrees: from 22:00:00 the newest second is night, and the night limits
  apply to an hour that is still 59 minutes of day. That is what the decree asks — the
  hour's average from then on is a night-time hour average.
- At 07:00 the obvious alternative, the set in force at the window's end, would judge the
  hour 06:00:01–07:00:00 against the day limits, 5 dB higher, though 59 minutes of it are
  night: a night hour certified against day limits. The stricter rule keeps the night limits
  until the last night second has left the window (08:00). A show still running at 07:00 is
  rare; the cost is an hour of the stricter limits, never a missed night exceedance.

The **headroom** is computed against the set in force once the horizon has passed: at
21:59:30 with a 60 s horizon the window will hold a night second, so the level offered is
the one that keeps the night limit, not the day one that is about to stop applying. The night
(9 h) is longer than any horizon (≤ 1 h), so the set after the horizon is night exactly
when a night second now in the window stays in it, or the horizon ends in the night.

**Clock changes.** Each second carries the period of its own local start, and a window is
the newest N real seconds, so a clock change never shortens or lengthens a window: the
autumn repeat of an hour is two hours of seconds, both classified by their wall time. In
Finland the clocks change at 03:00 / 04:00 local, inside the night, so the 22:00 and 07:00
boundaries never move relative to the seconds around them.

## The transfer FOH → the place

**Measurement** (once, at setup): a steady test signal (pink noise) through the system;
band Leq at FOH (the meter's mic) and at the place (the same mic moved, a second
calibrated mic, another rig's meter, or a calibrated recorder replayed), then the place's
**background** with the system silent. All three on one level scale (dB SPL). The FOH and
place figures need the **same steady signal at the same level**, not the same time: they
need not be simultaneous or on synchronised clocks. The operator names the place when
measuring (`BandTransferSet.place`). Per band:

    margin = L_place − L_background

- **≥ 10 dB: clean.** `D = L_FOH − L_place`; the background adds at most 0.41 dB.
- **3 … 10 dB: corrected.** The background's energy is subtracted first:
  `L_signal = 10·lg(10^(L_place/10) − 10^(L_background/10))`, `D = L_FOH − L_signal`
  (energy subtraction of the background; at a 3 dB margin it takes 3 dB off).
- **< 3 dB: unusable.** The transmitted part is below the background (`L_signal <
  L_background`), so the attenuation is only known to be **more than** `L_FOH −
  L_background`. The band is marked unusable and that bound is kept: it is the figure that
  never overstates how much the building takes away. The operator raises the test signal
  until the band is usable.
- Without a background measurement every band is **unchecked**: the difference as is, and
  the caption says so.

**Without a transfer** the limits are judged at the mic as typed (`BandLimitPlace::AtMic`):
a meter at the place is a monitor there, and a meter at FOH judges FOH limits the operator
typed. Whether the meter has a transfer is configuration, said in the dialog and by `ac2 spl
bands set`, not in the measurement view, which shows the levels and limits as judged. Where
the place cannot be reached, the operator can type an **estimated** attenuation per band
(`TransferOrigin::Estimated`, each band unchecked at the guess); it is judged like a
measured one and labelled estimated everywhere (`BandLimitPlace::Estimated`).

**FOH limits**: `L_lim,FOH(b) = L_lim,place(b) + D_b` (an unusable band's bound in place
of `D_b`, so the FOH limit is lower, i.e. safe). Judging FOH band Leq against FOH limits is
judging `L_FOH − D` against the place's limit, as `D` is a constant per band. The
attenuation is a property of the band, not of the weighting: a weighted window's level and
its limit both carry the same weighting offset, so the transfer moves every window's limits
alike.

**Predicted LAeq at the place** per second, from the FOH bands:

    L_A,place = 10·lg Σ_b 10^((L_b,FOH − D_b + A(f_b)) / 10)

with `A(f_b)` the IEC 61672-1 A weighting at the band's exact centre. A band's A weighting
varies across it (most at 20 Hz, about ±1.5 dB at the edges), which for a smooth spectrum
shifts the band's energy by a few tenths of a dB at the lowest bands, where the A-weighted
contribution is small anyway. Two figures are summed per second: the **estimate**, from
the bands with a measured attenuation, and **at most**, adding the unusable bands at their
bound. The predicted LAeq (`PredictedWindow`: its length, day and night limits and warn
margin; the Finnish preset's LAeq,1h ≤ 25 dB at night) is a rolling window over the
estimate, judged like any window; "at most" is shown beside it when the two differ. Judging
on the bound would let the place's own background in the mid bands (often 15–20 dB(A) of it) read as
music; showing it keeps the gap visible.

## §13 corrections

§13: an impulse correction of +5 or +10 dB and a narrowband (tonal) correction of +3 or +6
dB are added to the average levels of §12 mom 1, only for the time the character occurs. ac2
does not detect impulsiveness or tonality; the **operator types the correction** and when it
is in force. A rating level is the energy average of `L + K` over the period, so a second
with a correction in force enters every band window and the predicted LAeq with its energy
multiplied by `10^(K/10)` (`BandSecond::corrected`), whatever its weighting; the log keeps the raw levels and the
correction beside them.

## Presets

A preset replaces the windows, the shown bands and the predicted window, and keeps the §13
correction and the transfer; everything stays settable after it. Informational, not legal
advice.

- **`finland-545-lf`** — Liite 2 Taulukko 2: one LZeq 60 min window on 20 … 200 Hz, night
  (22–7) 20 Hz 74, 25 Hz 64, 31.5 Hz 56, 40 Hz 49, 50 Hz 44, 63 Hz 42, 80 Hz 40, 100 Hz 38,
  125 Hz 36, 160 Hz 34, 200 Hz 32 dB, day (7–22) each 5 dB higher; the predicted LAeq 60 min
  ≤ 25 dB at night (§12, music in rooms meant for sleeping). The limits hold at the place;
  the meter moves them to FOH with the transfer.
- **`finland-545-living-room`** — Liite 2 Taulukko 1: the predicted LAeq 60 min, day 35,
  night 30 dB; the bands 20 … 200 Hz shown in an LZeq 60 min window without limits.
- **`finland-545`** (the A/C/Z meter, `docs/design/leq.md`) — the hearing-damage limits of
  §12: LAeq,4h 100 dB, LAFmax 115 dB, LCpeak 140 dB, at the audience.

## Sources

**STM 545/2015**, Sosiaali- ja terveysministeriön asetus asuinrakennuksen ja muiden
oleskelutilojen terveydellisistä olosuhteista sekä ulkopuolisten asiantuntijoiden
pätevyysvaatimuksista (asumisterveysasetus), original text as published, retrieved
2026-10-08: <https://www.finlex.fi/api/media/statute/80244/mainPdf/main.pdf>.

- §12 mom 1 with Liite 2 Taulukko 2: "Pienitaajuisen sisämelun tunnin keskiäänitason
  toimenpiderajat nukkumiseen tarkoitetuissa tiloissa", the table above (night). "Päiväajan
  (klo 7–22) pienitaajuiselle melulle sovelletaan 5 dB suurempia arvoja kuin taulukossa 2."
- §12: at night (22–7), music noise clearly distinguishable from the background must not
  exceed LAeq,1h 25 dB in rooms meant for sleeping. Liite 2 Taulukko 1: living rooms LAeq
  day 35, night 30 dB.
- §12: "Kuulovaurion välttämiseksi melun äänitasot eivät saa ylittää LAeq,4h 100 dB, LAFmax
  115 dB tai LCpeak 140 dB."
- §13: impulse correction +5 or +10 dB, narrowband correction +3 or +6 dB, added for the
  time the character occurs.

## What is and isn't claimed

- The band filters meet IEC 61260-1 class 1 (`ac2_core::rta` tests); the band levels are
  the energy averages of their outputs. The instrument is not type-approved: the mics,
  their calibration and the interface are the operator's.
- The transfer is the **operator's measurement**, applied as measured. It holds for the
  positions, the system and the building as they were: moved speakers, a changed system
  EQ, open windows or a different room invalidate it, and ac2 cannot tell.
- A FOH prediction is **not a measurement at the place**. Only a measurement there, as
  the authorities make it, shows compliance; the prediction is the artist's guide to stay
  under it. Bands the transfer could not measure are shown as such, with the bound used.
- The day/night set follows the local wall time the host knows; the stricter-set rule for
  straddling windows is ac2's choice (above), not the decree's wording.
- §13 corrections are typed by the operator; ac2 does not judge the character of the noise.
- A weighted band window weights each band at its mid-band frequency (*Weighting a band*):
  exact for Z, within a few tenths of a dB for broadband signals otherwise.

## Stages

1. **Core** (done): `ac2_core::band_leq` — the band integrator over the 1/3-octave bank,
   per-band rolling windows judged on the leq rules, the day/night selection, the transfer
   with the background rules, FOH limits and the predicted LAeq; the `finland-545` preset.
2. **Daemon and protocol** (done): `SplConfig.bands` (`BandLeqConfig`: window length, day
   and night limits per band 20 … 200 Hz, warn margin, predicted LAeq limits, §13
   correction, the stored transfer with its per-band status; presets as
   `BandLeqPreset` `Finland545Lf` and `Finland545LivingRoom`, filled in by front ends), the band meter in
   the SPL job on the meter's second grid (`SplMeter::process_tapped` hands it the
   mic-curve corrected, unweighted signal), the period from the local wall time of each
   second (`ac2d::LocalClock`, injectable), alarms with the Leq windows' hysteresis
   (`AlarmSubject::band` / `predicted`), the `band_leq` frame each second, the per-second
   band log (`ac2_traces::band_log`, `<meter>.bands.csv` beside the SPL log in sessions and
   the autosave, same retention), windows rebuilt from it by wall time on a job start or a
   configuration change, and `spl.band_transfer` (`docs/protocol.md`, *Band meter*);
   `PROTO_VERSION` 26, session format 13. Decided on the way:
   - The transfer is stored in the meter's configuration, not the calibration store: it
     belongs to a FOH position and a place, not to a mic, and the calibration store's
     format is versioned per device, input and mic, so a change there would set aside
     operators' calibrations. It travels with the session and the autosave.
   - The position correction is not applied to the band meter: it is calibrated on A/C
     levels at the measuring position, and the band limits are at the place.
   - The transfer takes FOH, place and background band levels from spans of a band log
     (the same mic moved, or a second meter) or from typed or imported levels
     (`ac2_traces::band_levels`, `<Hz> <dB>` lines). There is no dedicated two-position
     capture job: the operator plays the test signal and names the spans (stage 4).
3. **Scene, UI, CLI** (done): `ac2_scene::band_leq` words and draws the `band_leq` frame —
   eleven bars 20 … 200 Hz with the limit line, the headroom ("≤ …") and "cooling down in
   …" per band, the worst band named in a headline with what to do ("63 Hz band Leq 3.2 dB
   over its limit · cooling down in 6 min 52 s"), the period (and "headroom for the night
   limits from 22:00" when the set changes within the horizon), the limits' place, the §13
   correction, the predicted LAeq at the place against its limit with "at most"; the SPL pane's
   fourth view (`SplMode::Bands`, G after meter + Leq when the meter has a band meter); the
   band rows of the Leq dialog (off / preset, window, impulse, narrowband, the transfer per
   band); `ac2 spl bands set|watch|transfer`. Decided on the way:
   - The CLI is `spl bands …`, a sibling of `spl leq …`, not more flags on `spl leq set`:
     the band meter has its own watch (bands, not windows), its own presets and the
     transfer, and `spl leq set --preset` replaces windows, which a band preset never does.
   - A transfer source on the CLI is a file of `<Hz> <dB>` lines or `METER@FROM..UNTIL`,
     times resolved on the CLI's host (local time of day, local or UTC date-time, `-30s`,
     `now`).
4. **Measuring the transfer** (done): `spl.band_log_get` reads a span of the band log back
   (the energy average a transfer takes, `ac2_traces::band_log::span_average`, shared by the
   daemon, the fake client and the reply; every `step`-th second; at most 3600 rows, else
   refused naming the step that fits); `ac2 spl bands log [--step N] [--levels-out FILE]`;
   the app's **band transfer step** over the Leq dialog (T on a band row: FOH, place and
   background spans marked with Space / Space or the last 1–9 minutes on the meter's clock,
   each read back, then Enter stores the transfer); `spl.band_transfer` refuses
   overlapping spans of one meter and a span of a meter without a band meter or log, saying
   what to do; a mic place and nothing judged at FOH without a transfer (both gone in stage
   5); `BandTransferSet.origin` and `ac2 spl bands estimate`; `ac2 rec import` (a
   recorder's 16/24/32-bit PCM or float WAV as a recording, so a calibrated recorder at the
   place without a cable replays, calibrates from its recorded tone and gives the place's
   spans); `PROTO_VERSION` 27. The fake client
   keeps band rows a test puts in (`Shared::band_rows`) and answers spans from them.
   Decided on the way:
   - A span is timed on the meter's clock (its newest frame's capture wall time in the
     app; the CLI's host clock for `METER@FROM..UNTIL`), so a rig elsewhere lines up with its
     own log.
   - A replay logs at the replay's wall time: at real-time pace file second t is the
     replay's start + t, so its spans are addressable; at fast pace they are not, and the
     guide says to replay a recorder in real time. Spans in file time (rather than replay
     wall time) are not offered.
   - The reply is bounded by refusing, not by decimating behind the caller's back.
   - Not yet: typing an estimated attenuation in the app (the CLI does it), the band
     history in the view, a recorder's file-time marks shown in the step.
5. **Band windows like the Leq windows** (done): `BandLeqConfig` = `windows` (up to eight
   `BandWindow`: length, weighting Z/A/C, `BandLimitSet` `Always` or `NightDay` with a day
   offset, warn margin), `bands` (the shown bands), `predicted` (`PredictedWindow` with its
   own length), the correction and the transfer, whose `place` the operator names
   (`receiving room` unless named). The `band_leq` frame carries one column per window and
   shown band (`leq`, `limit`, `allowed`, `recover`, `leq_flags`) and a `BandWindowState`
   per window; alarms name the window (`AlarmSubject::Band` {`duration`, `weighting`,
   `nominal`}); `PROTO_VERSION` 28, session format 14 (older sessions are set aside, not
   converted). The view stacks the windows, each a row of bars captioned with its name and
   period, and its headline follows the worst window; it says nothing about a transfer. The
   Leq dialog's band section is a table like the Leq windows': Off / On / a preset, the
   bands from … to and further bands, one row per window (length, weighting, day +dB,
   margin) with its limits per shown band beneath, Insert / Delete adding and removing a
   window. The CLI mirrors `spl leq set`: `ac2 spl bands set --windows z:60min,a:15min
   --bands 20hz..200hz,1khz --limit z:60min:63hz=42db --day-offset z:60min=5db --warn 3db`,
   `--preset` as before; `transfer --place-levels SOURCE --place NAME`, `estimate --place
   NAME`; `bands watch --json` gives a `windows` array. Decided on the way:
   - The mic place (FOH or at the place) is gone: without a transfer the limits are judged
     at the mic as typed, so a monitor at the place and FOH limits typed by hand both work,
     and the view no longer shows a "no band transfer" headline the stage cannot act on.
   - Weighting is an offset at the mid-band frequency (*Weighting a band*), not a filter
     per weighting: one bank, one unweighted log, any weighting later.
   - Wording names no room the operator did not name: "the place" in help, the operator's
     name (or "receiving room") in every line about it; a scene test greps for the rest.
   - Not yet: setting the predicted window other than by a preset (the dialog and the CLI
     keep it), per-band warn margins.
