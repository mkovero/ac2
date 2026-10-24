# Programme metering: input kinds, stereo pairs, PPM, correlation, goniometer

What a master stereo display (DK-Audio MSD600C / MSD600M class) gives a broadcast or
mastering desk, brought to the console feed of a PA: see what the system is being fed, not
only what the microphone hears. Decisions V1–V6 in `open-questions.md`. Revised 2026-10-23
after an independent review (ballistics, scales, gating, rotation, where settings live).

## What the operator gets

The Audio & I/O page already knows each input by name and role (reference, mic) and shows a
live meter per input. Two declarations are added there and everything else follows from
them:

- **Kind** of an input: *mic* (dB SPL via its calibration, as today), *line* (analogue,
  dBu when calibrated) or *digital* (dBFS, 0 dBFS is the real ceiling).
- **Pair**: two inputs declared left / right. The Inputs list shows them as one row with
  two bars and a correlation reading.

The input meters stop being an RMS bar with a peak tick and become peak programme meters
(PPM) in a ballistic the **rig** chooses (a daemon setting: every app looking at that rig
reads the same bar), with a flying peak, a maximum and an overs count since reset. A new
**Programme** pane shows the MSD trio big — bars, correlation, goniometer — for one pair or
a set of inputs, for FOH or the stage view. Loudness (BS.1770 / R128) joins the same pane in
a later stage.

Nothing of this is a measurement in the tree: the quantities belong to inputs, not to a
measurement, and run before a session opens (device preview) as the meters do today. The
math is the daemon's throughout; the app renders.

## 1. Input kinds (V1)

The mic role and the "domain" are one concept, so they are one type: what is on the input.

```rust
/// What an input carries, declared by the operator; the backend cannot tell.
pub enum InputKind {
    /// A measurement microphone: dB SPL through its calibration (decision K8's mic name
    /// and curve live here, so a line or digital input cannot carry a mic name at all).
    Mic { mic: Option<String>, curve: CurveChoice },
    /// Analogue line level: dBu through a line calibration (§1.1), dBFS without one.
    Line,
    /// A digital feed (AES, ADAT, Dante…): dBFS; 0 dBFS is the source's full scale.
    Digital,
}
```

`InputSetup` becomes `{ channel, kind, pair }`. Rules:

- **M** on the Audio & I/O page makes an input `Mic` (and **N** names it, as today); **L**
  makes it `Line`, **A** `Digital` (AES) — accepted by the operator 2026-10-23; the page
  already uses R M N S D P, Space and arrows, and these do not collide. Default for a
  new input: `Line`.
- Reference (**R**) is a role, not a kind: the loopback is `Line` or `Digital`.
- A `Line` input without a line calibration reads **dBFS** and its row says *dBFS*, not
  *uncalibrated*: a console feed metered in dBFS is complete as it is, and a nag on every
  line input (the loopback among them) would be noise. *dBu* appears once a line
  calibration exists, with its age, like a mic calibration's.
- Measurement kinds that are acoustic by definition — SPL meter, Leq, band Leq — take a
  `Mic` input only and refuse a `Line` or `Digital` one by name (*Stage is a line input;
  an SPL meter needs a microphone*). Transfer, spectrum, RTA and sweeps take any kind: a
  ratio or a dBFS spectrum of a line feed is a sound measurement too.
- Sensitivity calibration (`spl`) is offered on `Mic` only. A line calibration (§1.1) on
  `Line` only. Decision K8 (mic name per input) is superseded in form, not substance: the
  name and curve now live inside `Mic`. `Digital` is never calibrated: its reference is the word's full scale, and
  offering a calibration would invent a volt that does not exist.
- Backends cannot tell AES from analogue (a JACK port, an ASIO or Core Audio channel name
  say nothing reliable), so the kind is declared, like the MSD's input menu, and remembered
  per device with the roles (the app's `DeviceRoles` gains `kinds` and `pairs`).

### 1.1 Line calibration: dBu of 0 dBFS

The electrical calibration (`q7-calibration.md` §11) already measures the input's own volts
at full scale: a steady tone of RMS voltage `V` reading `L` dBFS gives `V_FS = V / 10^(L/20)`.
A line calibration stops there, with no capsule:

    dBu_FS = 20·lg(V_FS / 0.7746 V)          (dBu of 0 dBFS; 0 dBu = 1 mW into 600 Ω)

Check: an interface whose full scale is +13 dBu reads a +4 dBu tone at −9 dBFS. (The MSD's
"0 dBu indication, 1.55 V" is its scale's test point: 1.55 V is +6 dBu.)

It is its own calibration kind in the store, not a `CalBasis` variant of the sensitivity
calibration (that one is shaped around a capsule): `line` under `(device uid, input
channel)` with `v_fs`, `measured_dbfs`, `volts`, `calibrated_at`. New command `cal.line
{ channel, volts }` (the daemon measures the tone's level the way `cal.spl_electrical`
does; the operator types only the volts), CLI `ac2 cal line`, listed by `cal.list` beside
the mic calibrations and removed by `cal.delete`. A `Line` input without one reads dBFS and
its row says *dBFS* (§1).

The dBu scale of a calibrated line input runs `dBu_FS − 60 … dBu_FS` (full scale sits at
+13 … +24 dBu on real interfaces; nominal +4 dBu is in the upper part, the top 3 dB in the
warning colour).

## 2. Pairs (V2)

A pair is two inputs of the session declared `Left` and `Right`. One input belongs to at
most one pair; both inputs must be of the same kind, and `Mic` pairs are refused (a
correlation of two measurement mics means something else and would be read as a stereo
verdict). The pair is named by its inputs' names (*Main L + Main R*) until **N** names it on
the Audio & I/O page.

The Inputs list shows one row per pair: both bars, the correlation chip (§4), the overs of
the louder side, and the input numbers when the names lack them. A pair row in the list
selects like any child row (list click model); the Programme pane (§6) is where it opens
big.

### Key scope

Setup keys (R M L A N S, Space) exist only on the Audio & I/O page. Meter keys exist only
on Inputs-list rows and Programme panes: **R** there resets the row's or pane's maximum and
overs. The two lists never share a page, so R is Reference on one and reset on the other
without a chord; the hint bar of each page says which.

## 3. Peak programme meters (V3)

Every input meter — session inputs and device preview alike — runs a PPM beside the RMS it
has today.

```rust
/// The meter's dynamics. A typed choice: the standards' types, their constants in
/// `ac2_core::ppm`. Scales (Nordic, DIN, BBC, EBU) are faces on these detectors and are not
/// shipped: a bar reads dBFS or dBu.
pub enum Ballistic {
    /// Sample peak through 4× oversampling (BS.1770 Annex 2): what the converter or the
    /// link will actually see. The default.
    TruePeak,
    /// IEC 60268-10 Type I (the Nordic and DIN scales): quasi-peak, 5 ms integration
    /// (−2 dB on a 5 ms burst), 20 dB return in 1.7 s.
    Type1,
    /// IEC 60268-10 Type II (the BBC and EBU scales): quasi-peak, 10 ms integration,
    /// 24 dB return in 2.8 s.
    Type2,
    /// IEC 60268-17 VU: rectified average, 300 ms to 99 % of a steady tone. Offered on a
    /// calibrated `Line` input only: 0 VU is +4 dBu, which needs the volt.
    Vu,
}
```

The ballistic is a **rig setting** (`levels.set { ballistic, correlator, over_threshold }`,
in the daemon's rig settings beside the system max level), not an app preference: the
daemon computes the quantity, so the daemon owns the choice, and two apps on one rig — or a
Pi kiosk and a laptop — read the same bar. Another rig may choose otherwise. The daemon
runs **every** ballistic all the time and reports the chosen one, so a change reads a
settled detector at once, as `SplMeter` does for its weightings. The cost is the true-peak
interpolator (a 48-tap polyphase FIR, 48 multiply-adds a sample a channel: about a percent
of one Pi 4 core for eight channels) plus two two-rate detectors (one-pole attack,
linear-in-dB release) and one one-pole mean per channel.

Quasi-peak model: full-wave rectifier, first-order attack, exponential (linear-in-dB) return
at the stated rate. IEC 60268-10 specifies each type by a **table of tone-burst lengths and
the reading each must give, with a tolerance per row**, not by one integration time; the
`tools/refgen` vectors are that table plus a step down, and the acceptance is the standard's
per-row tolerance. A single attack time constant fitted to one row may miss the shortest
bursts by a few dB; if it does, the attack takes a second time constant rather than the
table losing rows. The table itself is not in hand (paid text) and is not reproduced here:
the constants quoted in the enum are the MSD spec sheet's, and the vectors are frozen only
after the standard has been read. VU: a first-order mean of |x| with the stated settling,
symmetric. True peak: the BS.1770
polyphase interpolator (48 taps at 4×; 2× at 96 kHz), max |x| per block, in dBTP.

Per input the frame carries (dBFS; the front end converts to dBu / dB SPL from the kind and
its calibration, so one wire quantity serves every scale):

| field | what |
|---|---|
| `rms` | as today |
| `ppm` | the chosen ballistic's reading; where VU is chosen but not offered (a mic or digital input, or an uncalibrated line input) the true-peak reading, and the row says *VU needs a line calibration · true peak shown* — a rig setting never blanks a bar |
| `true_peak` | dBTP, this frame |
| `max` | the highest reading of the chosen ballistic since the last reset (daemon; held per ballistic, since all run, so a ballistic change does not splice a Type I maximum onto true-peak readings) |
| `overs` | over events since the last reset (daemon) |
| `clip` | as today: samples at full scale (the converter's own clip) |

Two holds, two words: the **flying peak** above the bar is the app's, holds the last peak
for 1 s and falls (the MSD's "flying" marker; useful all show long); the **max** since
reset is the daemon's, so every client agrees and a reconnect keeps it. An **over** is
`true_peak` above the rig's over threshold (default 0 dBTP; R128 practice is −1 dBTP) and a
new event only after it fell 1 dB below it: a sustained clip counts once, a burst of clipped
notes counts the notes. *CLIP* (samples at full scale) and overs (true peak over a threshold
that may be below full scale) are different events and never share a word. `levels.reset
{ channels }` clears max and overs; the UI's **R** on an Inputs-list row sends it.

Preview (before a session opens): the app passes the device's remembered kinds and pairs in
`session.preview`, so preview bars read dBu on a calibrated line input, pair rows pair, and
the correlation runs — you can check the console feed before opening, as you find a mic by
tapping it today.

## 4. Correlation (V4)

Per pair the daemon computes the normalised cross-correlation at zero lag over an
exponential window:

    r = <l·r> / sqrt(<l²>·<r²>)          window 0.3 s or 1 s, a rig setting shown in seconds

(not *Fast* / *Slow*: those words mean 125 ms and 1 s on the SPL meter, and the same word
must not read as two quantities). Mono reads +1, uncorrelated (wide) stereo 0,
polarity-reversed −1. The reading is **gated per side**: when either side is below the gate
(rig setting, default −60 dBFS) the frame carries no correlation and the chip says which
side is quiet (*right quiet*) instead of a number — a dead channel must not read 0 and look
like wide stereo, and the MSD's forced zero does exactly that. Negative values are shown in
the warning colour (not mono-compatible: a sub fed the sum will cancel).

Why it matters at a PA: the L/R feed to a mono sub sum, a polarity-reversed return, a
"stereo" source that is really dual mono — all visible before anything is measured.

## 5. Goniometer (V5)

The audio vector oscilloscope: each sample pair plotted as a point, rotated 45° so that mono
stands upright and left leans left:

    x = (r − l) / √2        y = (l + r) / √2

A mono signal is a vertical line, a polarity-reversed one horizontal, a left-only one a
line leaning left at 45°, true stereo a cloud. **Auto gain**: the figure is normalised by
the pair's recent peak (exponential, τ = 1 s, floor −60 dBFS, so the range is 30 dB or more)
— the goniometer shows shape, the bars show level; it never reads a number.

**On the wire**: the daemon sends 256 point pairs per frame at the meter rate, quantised to
`i8` after auto gain; 512 bytes a frame. The pick is **not** a fixed stride: a periodic
signal at a stride that divides its period collapses to a few dots (1 kHz at 48 kHz, stride
6, is eight points). Each frame is split into 256 bins and one sample pair is drawn from a
random position in each bin, so a tone keeps its line and a cloud its shape. The scene keeps
a ring of the last ~1 s of points with their age; the renderer places them.

**The look — embers.** The field is dark. Each point is an ember: it lands bright and cools
along a blackbody ramp — white-yellow → orange → deep red → ash-grey → gone — on an
exponential decay (τ ≈ 0.4 s of display time; the one app-side Settings value of this note,
0.1 … 2 s, where 2 s gives a phosphor feel). Brightness at landing follows the sample's
instantaneous level within the auto-gain window, so loud transients land white and quiet
detail lands dull red; points that pile up add (additive blending into a persistence texture
that fades each frame), so a dense mono core glows while a wide, sparse halo smoulders. No
glow sprites or bloom passes: one point, one texel, additive, the fade does the rest — cheap
on an integrated GPU and on the Pi. The truth is in `ac2-scene` (position, age, landing
intensity per point); the colour ramp, the fade and the blending are `ac2-plot`'s, like
every other style. Grid: a faint square and the two diagonals, hidden by **T** like all plot
chrome.

Golden image: a 1 kHz tone left only, right only, mono, reversed, and uncorrelated noise, at
two decay settings; the left-only image must lean left.

## 6. The Programme pane (V6)

A new `PaneKind::Programme`. Its configuration is **what to show**, nothing else: a pair, or
up to eight inputs. This is not a measurement, so a pane source must be able to name it:

```rust
pub enum PaneContent {
    Meas(MeasId),
    Inputs(Vec<u16>),
    Pair(PairId),
}
```

(`multi-rig.md` §2 adds the rig to this.) Layout follows the MSD main screen — bars on the
right, correlation below the goniometer on the left — as a 1:1 split that becomes bars only
when no pair is shown, and collapses to bars + correlation chip on a short pane (the
goniometer needs height). Everything else comes from the I/O page (kinds, pairs, names,
calibrations) and the rig's settings (ballistic, correlator window, over threshold). **W** /
**F** make it the stage view as for every pane; **T** hides chrome; **R** resets max and
overs of what the pane shows.

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

## 7. Where the settings live

- **Rig settings** (daemon, `levels.set`, beside the system max level; `ac2 levels`):
  ballistic, correlator window (s), over threshold (dBTP), correlation gate (dBFS). The
  daemon computes these quantities, so it owns their parameters and every client reads
  alike. `levels.set` is a rig edit under decision R3: it names the rig rev it was made on
  and is refused with `conflict` when another client changed it first.
- **App Settings**: ember decay only — how a shape fades is how this operator likes to
  look, and no number depends on it.

## 8. Wire and persistence

- `SessionLevelsFrame` and `PreviewLevelsFrame` gain `ppm`, `true_peak`, `max`, `overs`
  per channel and `pairs: Vec<PairLevels { left, right, correlation: Option<f32>,
  quiet: Option<Side> }>`; `LevelsMeta` names the ballistic in force so a frame is
  self-describing. A `gonio` frame topic per pair carries the 256 quantised points.
  `levels.set`, `levels.reset { channels }`, `cal.line`; `session.preview` carries kinds and
  pairs. `PROTO_VERSION` bump.
- `InputSetup` becomes `{ channel, kind, pair }`; `session.inputs` (the command that sets
  them after `session.open`) refuses the rule violations of §1 and §2 by name, and pairs
  travel with it. Input setups live in the daemon's calibration store (`StoreFile.inputs`),
  not in session files, so the bump is the store's version: an older store loads with `Mic`
  where a mic name was and `Line` elsewhere, no pairs — and the app re-applies its remembered
  `DeviceRoles.mics` on load, so an unnamed mic (the M role without a name) stays `Mic` and
  its SPL meter is not refused.
- Line calibrations are a new entry kind in the calibration store, same file.
- `DeviceRoles` (app prefs) remembers kinds and pairs per device.
- Rig settings file gains the four values of §7.
- `PaneKind::Programme` and `PaneContent` in `ac2-proto` layout (the session's per-measurement
  layouts, decision P1) and the app's layout prefs.

## 9. What is and isn't claimed

- The ballistics are implemented to the named standards' burst tables and proven by
  tone-burst vectors; the meters are not type-approved instruments and the user guide says so
  once.
- dBu readings rest on a voltmeter and are as good as it; the row shows the calibration's
  age as mic calibrations do.
- Correlation is at zero lag: an inter-channel delay (a misaligned stereo pair) lowers it
  honestly; the meter does not search lags, and the pane does not guess why it is low.
- The goniometer shows shape at auto gain, never level; nothing in it is a number.
- Surround vector displays (the MSD "Jelly-Fish"), scale libraries, matrix routing and
  SRC/sync handling are the MSD's broadcast context and are not taken.

## 10. Sources

- DK-Audio *MSD600C User's Manual* and *MSD600M Technical Reference Manual*: the trio
  layout, correlator and oscilloscope behaviour, PPM scale set and dynamic-response table
  (the two PDFs are the operator's, not in the repository).
- IEC 60268-10 (PPM types I, II: burst tables, return times), IEC 60268-17 (VU): paid
  texts; constants checked against them before freezing vectors.
- ITU-R BS.1770-5 (K-weighting, gating, true peak), EBU R128, Tech 3341 (meter test
  signals), Tech 3342 (LRA): all free.
- `~/src/ac` `docs/loudness-bs1770-5.md` and its loudness module: a source of ideas and the
  Tech 3341/3342 vector list, never of code.

## Stages

1. **Kinds and pairs** (landed, `cal.line { channel, volts }` daemon-measured) — `InputKind`, `pair` in `InputSetup` and `DeviceRoles`, the I/O
   page keys and chips, pair rows in the Inputs list, the line calibration kind with
   `cal.line` and `ac2 cal line`, `session.preview` carrying kinds and pairs, calibration
   store and proto bumps, `ac2 session` CLI. Meters unchanged otherwise; a calibrated line input
   already reads dBu.
2. **PPM, correlation, goniometer, pane** — `ac2_core::ppm` (ballistics against the burst
   table, true peak, overs) and `ac2_core::stereo` (gated correlation, gonio points, auto
   gain) with refgen vectors; levels frames extended, `gonio` topic, `levels.set` /
   `levels.reset` and the rig settings; scene text for rows, chips and pane; `ac2-plot`
   ember renderer and golden images; `PaneKind::Programme` and `PaneContent`.
3. **Loudness** — `ac2_core::loudness` with Tech 3341/3342 vectors, the pane's column,
   `since … R resets`.

Each stage is one landed batch.
