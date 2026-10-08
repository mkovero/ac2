# Band Leq: low-frequency limits in a neighbour's bedroom

Status: stage 1 implemented (`ac2_core::band_leq`, the `finland-545` preset of
`docs/design/leq.md`); stages 2 and 3 below are not.

## What the operator and the artist get

A Finnish decree, STM 545/2015 (asumisterveysasetus), limits the low-frequency noise inside
rooms meant for sleeping per 1/3-octave band, 20 to 200 Hz, as the unweighted average over
an hour, and music noise at night as LAeq,1h 25 dB. Those limits hold **in the neighbour's
bedroom**, where nobody stands with a mic during a show. ac2 therefore measures at FOH and
carries the figures to the bedroom with a transfer measured once at setup: per band, how
much quieter the bedroom is than FOH. A band limit at FOH is the bedroom limit plus that
attenuation; the same transfer, with A weighting at the band centres, predicts the
bedroom's LAeq. The artist sees eleven bars (20 … 200 Hz) against their limit lines, the
headroom per band, the worst band named, the time to recover when one is over, and the
predicted bedroom LAeq against 25 dB.

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

**Bands: 1/3 octaves 20 Hz … 10 kHz, 28 of them.** The legal set is the first eleven
(20 … 200 Hz). The wider set is integrated because the predicted bedroom LAeq is A-weighted:
music through a building is dominated by the bass, but 250 Hz … 2 kHz still carry
A-weighted energy where the A weighting stops taking 10–30 dB away and the wall's loss has
not yet grown large, and leaving them out would understate the prediction. Above 10 kHz
the A-weighted contribution after a building element is negligible (its loss there is far
larger than the music's level above the bedroom's background), and stopping at 10 kHz
keeps every band below Nyquist at 44.1 kHz with margin (upper edge 11.2 kHz). The count is
fixed: a second is a `[f64; 28]` plus its measured time, never a `Vec`.

**Stored per second** (stage 2's log row): the wall time of its start, `m`, the 28 band
levels (dBFS, `10·lg(2·e/m)`, 0.01 dB), the §13 correction in force, the period it was
judged in (day / night, *Day and night*) and the sensitivity in force. The levels are stored
uncorrected; the correction is a column of its own, so a correction typed later or wrongly
can be reviewed against the raw record.

## Windows and judging per band

Each limited band has a **rolling window of one hour** (`Leq,1h`), one-second steps, built
on the same ring and sums as the A/C/Z windows (`ac2_core::leq::RollingLeq`, generic over
the slot: a band second is just another set of channels). Everything in
`docs/design/leq.md` *Windows*, *Limits*, *Judging a filling window* and the headroom /
recovery rules applies per band unchanged: the Leq over the measured time, a gap never
silence, f64 Neumaier sums recomputed exactly every window length, a filling window judged
on its energy budget (over only when certain, on course when the pace so far ends over),
the headroom as the highest steady level for the horizon, and "cooling down in …" as the
time to recover playing at the limit. Tests compare every band against brute-force sums
over random sequences with gaps.

**The worst band** is named: the most severe judgement (over, on course, near, ok), then
the band furthest above (or least below) its limit. That is the one the artist acts on —
typically one kick or bass note's band — so the stage view leads with it.

## Day and night

The decree's night is 22:00–07:00 and its day 07:00–22:00, local time; the day limits of
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

## The transfer FOH → bedroom

**Measurement** (once, at setup): a steady test signal (pink noise) through the system;
band Leq over the same period at FOH (the meter's mic) and in the bedroom (a second
calibrated mic, or the same mic moved while the signal stays the same), then the bedroom's
**background** with the system silent. All three on one level scale (dB SPL). Per band:

    margin = L_bedroom − L_background

- **≥ 10 dB: clean.** `D = L_FOH − L_bedroom`; the background adds at most 0.41 dB.
- **3 … 10 dB: corrected.** The background's energy is subtracted first:
  `L_signal = 10·lg(10^(L_bedroom/10) − 10^(L_background/10))`, `D = L_FOH − L_signal`
  (energy subtraction of the background; at a 3 dB margin it takes 3 dB off).
- **< 3 dB: unusable.** The transmitted part is below the background (`L_signal <
  L_background`), so the attenuation is only known to be **more than** `L_FOH −
  L_background`. The band is marked unusable and that bound is kept: it is the figure that
  never overstates how much the building takes away. The operator raises the test signal
  until the band is usable.
- Without a background measurement every band is **unchecked**: the difference as is, and
  the caption says so.

**FOH limits**: `L_lim,FOH(b) = L_lim,bedroom(b) + D_b` (an unusable band's bound in place
of `D_b`, so the FOH limit is lower, i.e. safe). Judging FOH band Leq against FOH limits is
judging `L_FOH − D` against the bedroom limit, as `D` is a constant per band.

**Predicted bedroom LAeq** per second, from the FOH bands:

    L_A,bedroom = 10·lg Σ_b 10^((L_b,FOH − D_b + A(f_b)) / 10)

with `A(f_b)` the IEC 61672-1 A weighting at the band's exact centre. A band's A weighting
varies across it (most at 20 Hz, about ±1.5 dB at the edges), which for a smooth spectrum
shifts the band's energy by a few tenths of a dB at the lowest bands, where the A-weighted
contribution is small anyway. Two figures are summed per second: the **estimate**, from
the bands with a measured attenuation, and **at most**, adding the unusable bands at their
bound. The predicted LAeq,1h is a rolling window over the estimate, judged against 25 dB
like any window; "at most" is shown beside it when the two differ. Judging on the bound
would let the bedroom's own background in the mid bands (often 15–20 dB(A) of it) read as
music; showing it keeps the gap visible.

## §13 corrections

§13: an impulse correction of +5 or +10 dB and a narrowband (tonal) correction of +3 or +6
dB are added to the average levels of §12 mom 1, only for the time the character occurs. ac2
does not detect impulsiveness or tonality; the **operator types the correction** and when it
is in force. A rating level is the energy average of `L + K` over the period, so a second
with a correction in force enters every band window and the predicted LAeq with its energy
multiplied by `10^(K/10)` (`BandSecond::corrected`); the log keeps the raw levels and the
correction beside them.

## Presets

- **`finland-545-lf`** — the band limits of Liite 2 Taulukko 2, unweighted Leq,1h in rooms
  for sleeping, night (22–7): 20 Hz 74, 25 Hz 64, 31.5 Hz 56, 40 Hz 49, 50 Hz 44, 63 Hz 42,
  80 Hz 40, 100 Hz 38, 125 Hz 36, 160 Hz 34, 200 Hz 32 dB; day (7–22) each 5 dB higher.
  In the bedroom; the meter places them at FOH with the transfer.
- **Music at night** — the predicted bedroom LAeq,1h ≤ 25 dB from 22 to 7 (§12). Living
  rooms (Liite 2 Taulukko 1: LAeq day 35, night 30 dB) are offered as the same predicted
  window with those limits when the transfer was measured into a living room.
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
- A FOH prediction is **not a measurement in the dwelling**. Only a measurement there, as
  the authorities make it, shows compliance; the prediction is the artist's guide to stay
  under it. Bands the transfer could not measure are shown as such, with the bound used.
- The day/night set follows the local wall time the host knows; the stricter-set rule for
  straddling windows is ac2's choice (above), not the decree's wording.
- §13 corrections are typed by the operator; ac2 does not judge the character of the noise.

## Stages

1. **Core** (done): `ac2_core::band_leq` — the band integrator over the 1/3-octave bank,
   per-band rolling windows judged on the leq rules, the day/night selection, the transfer
   with the background rules, FOH limits and the predicted LAeq; the `finland-545` preset.
2. **Daemon and protocol**: the band meter's configuration (band limits as presets
   `finland-545-lf` and the predicted music window, the transfer as a stored set with its
   per-band status, corrections with their time span), the per-second band log (columns
   above, saved with the session and the autosave, 48 h as the A/C/Z log), rebuilding the
   windows from the log by wall time, the band frame per second (per band: Leq, limit,
   verdict, headroom, recover time; the worst band; the predicted LAeq with its bound),
   a transfer measurement job (FOH and bedroom band Leq of the test signal, the
   background), `PROTO_VERSION` bump with `WIRE_LOCK` and fixtures.
3. **Scene, UI, CLI**: the artist view — eleven bars 20 … 200 Hz with the limit line, the
   headroom ("stay ≤ …") and "cooling down in …" per band, the worst band named, the day /
   night set and when it changes next, the predicted bedroom LAeq against 25 dB; `ac2 spl
   leq set --preset finland-545-lf`, `--transfer <file>`, and the transfer measurement
   from the CLI and a dialog.
