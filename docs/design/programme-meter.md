# Programme metering: input domains, stereo pairs, PPM, correlation, goniometer

What a master stereo display (DK-Audio MSD600C / MSD600M class) gives a broadcast or
mastering desk, brought to the console feed of a PA: see what the system is being fed, not
only what the microphone hears. Decisions V1–V6 in `open-questions.md`.

## What the operator gets

The Audio & I/O page already knows each input by name and role (reference, mic) and shows a
live meter per input. Two declarations are added there and everything else follows from
them:

- **Domain** of a non-mic input: *line* (analogue, dBu when calibrated) or *digital* (dBFS,
  0 dBFS is the real ceiling). A mic is its own domain (dB SPL via its calibration).
- **Pair**: two inputs declared left / right. The Inputs list shows them as one row with
  two bars and a correlation reading.

The input meters stop being an RMS bar with a peak tick and become peak programme meters
(PPM) in a ballistic chosen once for the whole app, with a flying peak, hold, and an overs
count. A new **Programme** pane shows the MSD trio big — bars, correlation, goniometer — for
one pair or a set of inputs, for FOH or the stage view. Loudness (BS.1770 / R128) joins the
same pane in a later stage.

Nothing of this is a measurement in the tree: the quantities belong to inputs, not to a
measurement, and run before a session opens (device preview) as the meters do today.

## 1. Domains (V1)

```rust
/// What an input carries, declared by the operator; the backend cannot tell.
pub enum InputDomain {
    /// A measurement microphone: dB SPL through its calibration (as today).
    Mic,
    /// Analogue line level: dBu through a line calibration (§1.1), dBFS without one.
    Line,
    /// A digital feed (AES, ADAT, Dante…): dBFS; 0 dBFS is the source's full scale.
    Digital,
}
```

`InputSetup` (decision K8: channel, mic, curve) gains `domain` and `pair` (§2). Rules:

- `Mic` is what the **M** role sets; `mic` and `curve` are only meaningful on it. `Line` and
  `Digital` have no mic name and no curve; setting one is refused, not warned about later.
- Reference (**R**) is a role, not a domain: a reference is `Line` or `Digital`, which only
  decides how its own meter reads. Default for a new non-mic input: `Line`.
- Sensitivity calibration (`spl`) is offered on `Mic` only. A line calibration (§1.1) is
  offered on `Line` only. `Digital` is never calibrated: its reference is the word's full
  scale, and offering a calibration would invent a volt that does not exist.
- A JACK port, an ASIO or Core Audio channel name says nothing reliable about AES versus
  analogue, so the domain is declared, like the MSD's input menu, and remembered per device
  with the roles (the app's `DeviceRoles`, which gains `domains` and `pairs`).

### 1.1 Line calibration: dBu of 0 dBFS

The electrical calibration (`q7-calibration.md` §11) already measures the input's own volts
at full scale: a steady tone of RMS voltage `V` reading `L` dBFS gives `V_FS = V / 10^(L/20)`.
A line calibration stops there, with no capsule:

    dBu_FS = 20·lg(V_FS / 0.7746 V)          (dBu of 0 dBFS; 0 dBu = 1 mW into 600 Ω)

Check: the MSD's analogue reference is 1.55 V at the +6 dBu mark (its spec sheet's "0 dBu
indication, 1.55 V input" is the scale's test point). An interface whose full scale is
+13 dBu reads a +4 dBu tone at −9 dBFS. Stored as a `line` calibration under the same key
shape as `spl` (`device uid, input channel`, no mic name), `dbu_fs` and `calibrated_at`;
`cal.list` shows it beside the mic calibrations and `cal.delete` removes it. A `Line` input
without one reads dBFS, like an uncalibrated mic, and its row says *uncalibrated*.

## 2. Pairs (V2)

A pair is two inputs of the session declared `Left` and `Right`. One input belongs to at
most one pair; both inputs must share a domain, and `Mic` pairs are refused (a correlation
of two measurement mics means something else and would be read as a stereo verdict). The
pair is named by its inputs' names (*Main L + Main R*) until **N** names it.

The Inputs list shows one row per pair: both bars, the correlation chip (§4), the overs of
the louder side, and the input numbers when the names lack them. A pair row in the list
selects like any child row (list click model); the Programme pane (§6) is where it opens
big.

## 3. Peak programme meters (V3)

Every input meter — session inputs and device preview alike — runs a PPM beside the RMS it
has today. The ballistic is **one app-wide setting**, like choosing a scale on the MSD: the
operator reads every bar the same way.

```rust
/// The meter's dynamics. A typed choice: the standards' names, their constants in
/// `ac2_core::ppm`.
pub enum Ballistic {
    /// Sample peak through 4× oversampling (BS.1770 Annex 2): what the converter or the
    /// link will actually see. The default.
    TruePeak,
    /// IEC 60268-10 Type I (Nordic N9 / DIN 45406): quasi-peak, −2 dB on a 5 ms burst,
    /// 20 dB return in 1.7 s.
    Nordic,
    /// IEC 60268-10 Type IIa (BBC) / IIb (EBU): quasi-peak, 10 ms integration,
    /// 24 dB return in 2.8 s.
    Bbc,
    /// ARD Pflichtenheft 3/6 (German broadcast): −3 dB on a 3 ms burst, 20 dB in 1.5 s.
    Din,
    /// IEC 60268-17 VU: rectified average, 300 ms to 99 % of a steady tone, no return
    /// asymmetry.
    Vu,
}
```

The constants above are the standards' as the MSD spec sheet states them (its sheet gives
IEC 268-10 as 5 ms / −2 dB, 20 dB in 2.0 s; Type IIa's 24 dB / 2.8 s is from the standard).
Before the golden vectors are frozen the attack and return of each type are checked against
the standard's text; the vectors (`tools/refgen`) are tone bursts of the stated lengths and
a step down, so a wrong constant fails a test rather than hiding in a comment.

Quasi-peak model: full-wave rectifier, first-order attack with the time constant that gives
the stated burst reading, exponential (linear-in-dB) return at the stated rate. VU: a
first-order mean of |x| with the stated settling, symmetric. True peak: the BS.1770
polyphase interpolator (48 taps at 4×; 2× suffices at 96 kHz), max |x| per block, reported
in dBTP; it is also the quantity the overs count.

Per input the frame carries (dBFS, converted to dBu / dB SPL by the front end from the
domain and its calibration, so one wire quantity serves every scale):

| field | what |
|---|---|
| `rms` | as today |
| `ppm` | the ballistic's reading |
| `true_peak` | dBTP, this frame |
| `held` | the highest `ppm` since the last reset (the flying marker's hold) |
| `overs` | count of over events since the last reset |
| `clip` | as today |

An **over** is `true_peak` above the over threshold (Settings; default 0 dBTP, −1 dBTP in
the R128 preset) and a new event only after it fell 1 dB below it: a sustained clip counts
once, a burst of clipped notes counts the notes. Hold and overs live in the daemon
(`levels.reset` clears them) so that every client reads the same hold and a reconnect does
not forget it; the UI's **R** on an input row sends the reset.

The bar's scale is the domain's: `Digital` −60 … 0 dBFS with the top 3 dB in the warning
colour, `Line` the same in dBu when calibrated, `Mic` its SPL scale. No scale library: one
scale per domain, the ballistic chosen by name.

## 4. Correlation (V4)

Per pair the daemon computes the normalised cross-correlation at zero lag over an
exponential window:

    r = <l·r> / sqrt(<l²>·<r²>)          τ = 0.3 s (Fast) or 1.0 s (Slow), a Settings choice

Mono reads +1, uncorrelated (wide) stereo 0, polarity-reversed −1. The reading is **gated**:
when both sides are below the gate (Settings; default −60 dBFS) the frame carries no
correlation (`Option`), and the chip shows a dim centre mark and *quiet* instead of a number
— the MSD forces zero, which reads as "wide stereo"; `no-misreadable-verdicts` says a
readout without evidence says so. Negative values are shown in the warning colour (not
mono-compatible: a sub fed the sum will cancel).

Why it matters at a PA: the L/R feed to a mono sub sum, a polarity-reversed return, a
"stereo" source that is really dual mono — all visible before anything is measured.

## 5. Goniometer (V5)

The audio vector oscilloscope: each sample pair plotted as a point, rotated 45° so that mono
stands upright:

    x = (l − r) / √2        y = (l + r) / √2

A mono signal is a vertical line, a polarity-reversed one horizontal, a one-sided one a line
at 45°, true stereo a cloud. **Auto gain**: the figure is normalised by the pair's recent
peak (exponential, τ = 1 s, floor −60 dBFS, so the range is 30 dB or more) — the goniometer
shows shape, the bars show level; it never reads a number.

**On the wire**: the daemon decimates to 256 point pairs per frame at the meter rate (every
`k`-th sample of the block, not a random pick, so a periodic signal keeps its figure),
quantised to `i8` after auto gain; 512 bytes a frame. The scene keeps a ring of the last
~1 s of points with their age; the renderer places them.

**The look — embers.** The field is dark. Each point is an ember: it lands bright and cools
along a blackbody ramp — white-yellow → orange → deep red → ash-grey → gone — on an
exponential decay (τ ≈ 0.4 s of display time, a Settings choice between 0.1 and 2 s for a
"phosphor" feel at 2 s). Brightness at landing follows the sample's instantaneous level
within the auto-gain window, so loud transients land white and quiet detail lands dull red;
points that pile up add (additive blending into a persistence texture that fades each
frame), so a dense mono core glows while a wide, sparse halo smoulders. No glow sprites or
bloom passes: one point, one texel, additive, the fade does the rest — this is cheap on an
integrated GPU and on the Pi. The truth is in `ac2-scene` (position, age, landing intensity
per point); the colour ramp, the fade and the blending are `ac2-plot`'s, like every other
style. Grid: a faint square and the two diagonals, hidden by **T** like all plot chrome.

Golden image: a 1 kHz tone left only, right only, mono, reversed, and uncorrelated noise, at
two decay settings.

## 6. The Programme pane (V6)

A new `PaneKind::Programme`. Its configuration is **what to show**, nothing else: a pair, or
up to eight inputs. Layout follows the MSD main screen — bars on the right, correlation
below the goniometer on the left — as a 1:1 split that becomes bars only when no pair is
shown, and collapses to bars + correlation chip on a short pane (the goniometer needs
height). Everything else comes from the I/O page (domains, pairs, names, calibrations) and
Settings (ballistic, correlator speed, over threshold, ember decay). **W** / **F** make it
the stage view as for every pane; **T** hides chrome; **R** resets hold and overs of what
the pane shows.

Opened from the picker like any pane, or from a pair row with **Enter** (the row brings its
own pane the way a measurement row does). Saved in the layout prefs and the session's panes
by its input set; a session loaded on a device without those inputs shows the pane empty
with *inputs 3+4 not in this session*.

### 6.1 Loudness (stage 3)

The same pane carries BS.1770-5 / EBU R128 for a pair (channel weights 1, 1) or a single
input: momentary (400 ms), short-term (3 s), integrated with the −70 LKFS absolute and
−10 LU relative gates, loudness range (EBU Tech 3342) and max true peak, all since the last
reset (*since 20:14 · R resets*, the SPL meter's wording). The K-weighting, gating and LRA
math is pure `ac2-core`; its golden vectors are EBU Tech 3341 / 3342's own published test
signals through `tools/refgen` (free, unlike the ballistics standards). Shown as a column of
numbers beside the bars; the integrated value is the only one that needs a start, and the
reset is it.

Not in this stage: a loudness log or session persistence of the integrated value. If a
show's integrated loudness turns out to be wanted after the fact, it becomes a measurement
with a log like the SPL meter's; the pane's numbers stay the live view.

## 7. Settings

Under the existing Settings view, one section *Programme meters*: ballistic (§3),
correlator Fast / Slow (§4), over threshold in dBTP (§3), ember decay (§5). App-wide, saved
with the app's prefs, not in sessions (`settings.md` §5: how the operator likes to read is
theirs, not the session's).

## 8. Wire and persistence

- `SessionLevelsFrame` and `PreviewLevelsFrame` gain `ppm`, `true_peak`, `held`, `overs`
  per channel and `pairs: Vec<PairLevels { left, right, correlation: Option<f32> }>`;
  `LevelsMeta` names the ballistic in force so a frame is self-describing. A `gonio` frame
  topic per pair carries the 256 quantised points. `levels.reset {channels}`. `PROTO_VERSION`
  bump.
- `InputSetup` gains `domain` and `pair`; `session.open` refuses the rule violations of §1
  and §2 by name. Session format bump; older sessions load with `Line` on every non-mic
  input and no pairs.
- Line calibrations are a new entry kind in the calibration store, same file.
- `DeviceRoles` (app prefs) remembers domains and pairs per device beside mic names.
- Settings: the four values of §7.
- `PaneKind::Programme` in `ac2-proto` layout and the app's layout prefs.

## 9. What is and isn't claimed

- The ballistics are implemented to the named standards' stated constants and proven by
  tone-burst vectors; the meters are not type-approved instruments and the user guide says so
  once.
- dBu readings rest on a voltmeter and are as good as it; the row shows the calibration's
  age as mic calibrations do.
- Correlation is at zero lag: an inter-channel delay (a misaligned stereo pair) lowers it
  honestly; the meter does not search lags. A pair that reads low *and* measures a delay in
  the transfer function is the operator's to interpret, and the Programme pane does not
  guess.
- The goniometer shows shape at auto gain, never level; nothing in it is a number.
- Surround vector displays (the MSD "Jelly-Fish"), scale libraries, matrix routing and
  SRC/sync handling are the MSD's broadcast context and are not taken.

## 10. Sources

- DK-Audio *MSD600C User's Manual* and *MSD600M Technical Reference Manual*: the trio
  layout, correlator and oscilloscope behaviour, PPM scale set and dynamic-response table
  (the two PDFs are the operator's, not in the repository).
- IEC 60268-10 (PPM types I, II), IEC 60268-17 (VU), ARD Pflichtenheft 3/6 (DIN scale):
  ballistic definitions; paid texts, constants to be checked against them before freezing
  vectors.
- ITU-R BS.1770-5 (K-weighting, gating, true peak), EBU R128, Tech 3341 (meter test
  signals), Tech 3342 (LRA): all free.
- `~/src/ac` `docs/loudness-bs1770-5.md` and its loudness module: a source of ideas and the
  Tech 3341/3342 vector list, never of code.

## Stages

1. **Domains and pairs** — `InputDomain`, `pair` in `InputSetup` and `DeviceRoles`, the I/O
   page keys and chips, pair rows in the Inputs list, line calibration kind, session and
   proto bumps, `ac2 session` / `ac2 cal` CLI. Meters unchanged otherwise.
2. **PPM, correlation, goniometer, pane** — `ac2_core::ppm` (ballistics, true peak,
   overs) and `ac2_core::stereo` (correlation, gonio points, auto gain) with refgen
   vectors; levels frames extended, `gonio` topic, `levels.reset`; Settings section; scene
   text for rows, chips and pane; `ac2-plot` ember renderer and golden images;
   `PaneKind::Programme`.
3. **Loudness** — `ac2_core::loudness` with Tech 3341/3342 vectors, the pane's column,
   `since … R resets`.

Each stage is one landed batch; stage 1 alone already makes the I/O page say what each
input is and reads dBu on a calibrated line input.
