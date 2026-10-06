# Room parameters (ISO 3382-1) from the sweep impulse response

Status: implemented (`ac2_core::room`, computed by the sweep analysis, `SweepData.room` on the
wire, `ac2 ir capture` / `ac2 ir metrics`, the table under the sweep's IR view in the app, the
`room_metrics` line and table in the ac2 CSV export). Answers PLAN.md §3.7 "ETC, Schroeder,
T20/T30/EDT, C50/C80/D50 per band" and §5.5 "ISO 3382-1 per band with own trigger, noise
truncation + tail correction".

## What is computed

For the broadband IR as captured (its band is the sweep's), every IEC 61260-1 octave band
63 Hz…8 kHz and every one-third-octave band 50 Hz…10 kHz whose edges lie inside the sweep's
excited range (the fades excluded):

| parameter | definition |
|---|---|
| EDT | least-squares fit of the decay curve over 0…−10 dB, time for 60 dB |
| T20 | fit over −5…−25 dB, time for 60 dB |
| T30 | fit over −5…−35 dB, time for 60 dB |
| C50, C80 | 10·lg(early / late energy), split 50 / 80 ms after the onset |
| D50 | early (50 ms) / total energy |
| decay range | depth of the decay curve where the decay meets the noise |
| curvature | 100·(T30/T20 − 1) %; above 10 % the decay is not straight (T30 shown with `*`) |

The IR used is the averaged deconvolved response at the full sample rate (not the stored,
decimated display IR), from at most 100 ms before the arrival (room for the band filters'
pre-ringing, after H2's impulse) to 10 ms before the end of the silence after the sweep.
It ends there because a lag beyond the recorded silence pairs the sweep with samples the
record does not have: the deconvolved noise thins out with lag and would read as decay. The
silence is the request's `tail` (1…20 s; the dialog offers 1, 2, 4, 8 s, the CLI `--tail`);
it must hold the decay and some noise after it. Lundeby's noise estimate needs that noise.

## Method

**Band filtering, backwards in time.** Each band is the bank's IEC 61260-1 class 1
Butterworth band-pass (order 3 per side, `rta::full_rate_band`: the bank's design at the
full rate, no decimators) run over the time-reversed IR, and the output reversed back
(Jacobsen & Rindel 1987). A causal band filter rings after every arrival and stretches the
measured decay by its own; reversed, the ringing lands before the arrival and the decay after
the onset is the room's. The ensemble-mean band energy of an exponential decay through the
reversed filter is exactly exponential from the onset on (test
`backwards_filtering_keeps_the_decay`: forward filtering lengthens EDT by > 40 % at
B·T = 8, reversed it is within 1 %).

**Onset (own trigger per band).** ISO 3382-1 puts the start of the response where it first
rises to within 20 dB of its peak. Each band takes its own trigger on its energy, but never
earlier than the broadband trigger: what the reversed filter shows before the broadband onset
is later sound moved earlier by the filter's pre-ringing. Its energy is counted in the first
sample, its time is not (the decay starts at the onset).

**Noise truncation and tail correction (Lundeby et al. 1995).** On the band energy from the
onset: average over 20 ms intervals; noise from the last 10 %; a line from the peak to 10 dB
above the noise gives the first crossing point; re-average with 5 intervals per 10 dB of that
slope; then iterate (at most 5 times): noise from 5 dB of decay past the crossing point (at
least the last 10 %), late decay line over 5…25 dB above the noise, new crossing point where
it meets the noise, until it moves less than one interval. The Schroeder backward integral
(Schroeder 1965) runs from the crossing point back to the onset, starting from the energy the
late decay line would carry after the crossing point (a geometric series), so the curve does
not bend down at its end.

**Energy ratios, window before filtering.** ISO 3382-1 suggests it, so does the ODEON paper
below: the broadband IR is cut at its onset + 50 / 80 ms, each piece is band-filtered forwards
with 20/B seconds of tail, and all of each piece's output energy is counted; the filter's delay
and ringing cannot move early energy into the late part. The late part ends at the band's
crossing point plus the same tail correction.

## Refusals (never a misleading number)

| refusal | when | shown as |
|---|---|---|
| `insufficient_range` | decay range below what the parameter needs: EDT, C50, C80, D50 20 dB; T20 35 dB; T30 45 dB (ISO 3382-1: the bottom of the evaluation range plus 10 dB) | `noise` |
| `filter_limited` | bandwidth × decay time below 8: the energy the reversed filter moves to the onset is a step at the top of the curve, and one band of one response has only about B·T degrees of freedom | `short` |
| `no_decay` | no energy, no falling curve, or a curve that falls through its range in under 2 ms (a step: the direct sound alone, e.g. an anechoic path) | `—` |

The legend under the table says what each word means; `ac2 ir metrics --json` carries the
refusal with its numbers (`{"type":"refused","reason":{"type":"insufficient_range",
"range":40.2,"needed":45.0}}`).

## Validation and accuracy

Expected values are analytic or from numpy/scipy, never from `ac`.

| test | case | quantity | tolerance | achieved |
|---|---|---|---|---|
| `room::tests::broadband_matches_the_analytic_room` | exponentially decaying Gaussian noise, T = 0.3 / 0.8 / 2 s, direct sound, 3 seeds, 75 dB SNR | EDT / T20 / T30 broadband vs the noise-free envelope's own fit | 6 % / 2.5 % / 2 % | passes (typ. < 1 %) |
| same | | C50 / C80, D50 | ±0.5 dB, ±0.02 | passes (typ. < 0.3 dB) |
| `octave_bands_are_unbiased` | 16 rooms, T = 0.4 / 1.2 s | mean T20 / T30 per octave | 3 % from 500 Hz, 8 % below | passes |
| same | | mean EDT; mean C80 | 5 % / 15 %; 0.5 / 1.5 dB | passes |
| same | | each room's T30 from 1 kHz | 12 % | passes |
| `noise_is_truncated_and_corrected` | SNR 70, 55, 46, 38 dB | crossing point vs analytic `T·SNR/60` | 10 % | passes |
| same | | decay times where given | 3 % (5 % within 10 dB of the limit) | passes; T30 refused at 38 dB, given at 46 dB |
| `a_double_slope_decay_shows_curvature` | 0.3 s for 25 dB then 1.5 s | EDT / T20 / T30 vs envelope; curvature (31 %) | 6 % / 4 % / 4 %; ±8 points | passes |
| `filter_and_schroeder_match_refgen` | golden set `room_schroeder_octaves` (`tools/refgen/sets/room.py`: scipy `butter` + `sosfilt` reversed, `numpy.polyfit`) | Schroeder curve every 1 ms, EDT / T20 / T30, C50 / C80 per octave | 1e-6 dB, 1e-7 rel., 1e-6 dB | passes |
| `sweep::tests::a_room_keeps_its_parameters_through_the_sweep` | a 0.5 s room through the whole sweep chain (ESS, loopback, deconvolution) vs the room's own IR analysed directly | decay times per octave; broadband; C80 | 0.5 %; 3 %; 0.3 dB | passes |
| `ac2d/tests/sweep.rs` `a_sweep_in_a_hall_reads_its_reverberation_time` | daemon + fake rig, Schroeder reverberator T60 = 0.8 s (every comb's loop gain set for 60 dB in 0.8 s) | T20 / T30, octaves 250 Hz…4 kHz and broadband | 8 % | 0.75…0.81 s |

Single bands of a single IR scatter (the statistical uncertainty of any IR measurement,
Lundeby et al. discuss it): at 63–250 Hz one room's T20 is within about ±30 %, its EDT
within ±50 % at short decay times. The app and CLI show one measurement; averaging
positions is the operator's job (and a later spatial average).

## Display

`ac2_scene::room` builds every string: band columns (nominal names `63`…`8k`, `1.25k`, `All`
last), rows `EDT (s)`, `T20 (s)`, `T30 (s)`, `C50 (dB)`, `C80 (dB)`, `D50 (%)`, `Range (dB)`,
refused cells as words (dimmed), the legend for the words and marks that occur, and the caption
`Room (ISO 3382-1) · octave bands · decay to 1980 ms`. In the app the sweep pane's IR view
(Shift+I) draws the octave table under the IR when the pane is at least 140 px taller than the
table; a pane too narrow drops outer bands (broadband stays) and says how many are hidden.
The pane's third view (G: response → IR → room) is the table alone under the banners, its
font the largest (up to twice the theme's) at which every band and the legend fit; a pane
too small even for the small font keeps the narrow-pane rule (`room::room_scene`). The
CLI prints the octave table after the distortion summary of `ac2 ir capture` and on
`ac2 ir metrics <trace> [--third]`; `--json` gives the full `RoomAcoustics`.

## Storage

`SweepData.room` (`RoomAcoustics`, nil only for an import from a v2 export) on the wire
(PROTO_VERSION 18). The ac2 CSV export v3 adds `# room_metrics: {…}` (the JSON
`RoomAcoustics`, what import reads) and, after the IR table, a readable table of comment lines
(`# band_hz,edt_s,…`; refused values `refused:<why>`). Sessions (format 9) keep it in the
trace's CSV.

## Sources

- ISO 3382-1:2009, *Acoustics — Measurement of room acoustic parameters — Part 1: Performance
  spaces* (decay ranges, evaluation ranges, onset at 20 dB below the peak, C50/C80/D50). Not
  in the repository.
- M. R. Schroeder, "New method of measuring reverberation time", J. Acoust. Soc. Am. 37,
  409–412 (1965), doi:10.1121/1.1909343.
- A. Lundeby, T. E. Vigran, H. Bietz, M. Vorländer, "Uncertainties of measurements in room
  acoustics", Acustica 81(4), 344–355 (1995).
- F. Jacobsen, J. H. Rindel, "Time reversed decay measurements", J. Sound Vib. 117(1),
  187–190 (1987).
- C. L. Christensen, G. Koutsouris, J. H. Rindel, "The ISO 3382 parameters: can we simulate
  them? Can we measure them?", International Symposium on Room Acoustics (ISRA), Toronto
  2013 (reversed filtering for decays, window-before-filtering for the ratios, the 20 dB
  onset rule).

## Limits and open questions

- One-third-octave bands at 50–100 Hz are refused (`short`) unless the decay is long: their
  bandwidth is 12–23 Hz.
- The fake rig's reverberator builds its diffuse field over tens of ms, so its EDT reads
  longer than its T60; T20/T30 are what that test checks.
- No STI/STIPA, no spatial averaging over positions, no per-band onset shown in the app.
