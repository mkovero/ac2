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
table losing rows. The table as published secondary (the standard's own text is paid; the
BBC's PPM conformance files and the standards summaries agree on these rows, 5 kHz bursts,
reading below the steady tone):

| burst | Type I | Type II |
|---|---|---|
| 100 ms | 0 dB | 0 dB |
| 10 ms | −1 dB | −2 dB |
| 5 ms | −2 dB | −4 dB |
| 3 ms | −4 dB | — |
| 1.5 ms | — | −9 dB |
| 0.5 ms | — | −17 dB |
| 0.4 ms | −15 dB | — |

Return: Type I 20 dB in 1.7 s, Type II 24 dB in 2.8 s, linear in dB. Acceptance in ac2 (the
standard's own tolerances are not in hand): ±1 dB on the 100, 10 and 5 ms rows, ±2 dB on the
shorter ones, and the return within ±0.3 s. A single first-order attack cannot hold both ends
of a column (fitted to 10 ms it over-reads the 0.4 ms burst by 3 dB), so the attack is two
first-order charges in parallel, a fast one carrying most of the reading and a slow one the
rest (`reading = a·(1−e^(−T/τ₁)) + (1−a)·(1−e^(−T/τ₂))`); `tools/refgen` fits `a, τ₁, τ₂`
per type to the table by least squares, the constants are frozen in both the generator and
`ac2_core::ppm`, and the Rust is tested against the generator's burst vectors (parity) and
against the table (acceptance) separately. The detector input is the full-wave rectified
sample; on a steady sine the reading is the sine's peak (the release is far slower than a
period), so the wire carries dBFS of the peak as `true_peak` does. VU: a first-order mean of |x| with the stated settling,
symmetric. True peak: the BS.1770-5
Annex 2 interpolator (the 48-tap, 4-phase FIR the annex prints), max |x| per block, in dBTP;
4× at every sample rate (at 96 kHz that is more than the annex asks and costs eight
channels about two percent of a Pi core, which buys one code path and one set of vectors).

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
detail lands dull red; points that pile up add (additive blending), so a dense mono core glows while a wide, sparse halo smoulders. No
persistence texture, glow sprites or bloom passes: the scene's ring already holds every
live point with its age, so each frame draws the ring once as small additive points whose
colour and brightness come from the age — a few thousand points at most, cheap on an
integrated GPU and on the Pi, and a golden image is reproducible from a ring alone. The truth is in `ac2-scene` (position, age, landing
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

BS.1770-5 / EBU R128 loudness of the programme, beside the bars, by the definitions of EBU
Tech 3341 V4 and Tech 3342 V4 (2023). Readouts on the absolute scale only (LUFS; LU for the
range). ac2 does **not** claim EBU Mode: it has no relative readout (LU re −23 LUFS), no
loudness bar with the +9 / +18 scales and no start / pause / continue (§9).

**Sources.** The daemon computes loudness for every declared pair whose both inputs are
captured and are line or digital (stereo, channel weights 1, 1), and for every captured
line or digital input not in a pair (one channel, weight 1). An input of a declared pair
whose partner is not captured has no source. Mic inputs get none: a LUFS
figure on a measurement mic depends on its preamp gain and would be misread beside its
dB SPL. Computing every source always is cheap (two biquads per sample per channel; the
true peak already runs for the meters) and needs no subscription bookkeeping: every client
reads the same numbers. A one-channel source is named *mono* where it is shown: BS.1770
weighs a single channel 1, so a mono signal on one input reads 3 LU below the same signal on
both inputs of a pair; that is the standard's answer. When `session.inputs` changes a kind
or a pair, the sources that changed are dropped and the new ones start afresh (their since
line shows it). A new source's sub-block grid starts at the first sample of the first audio
block metered after the change (or after `session.open`, or after a preview's pairs are
re-declared), with zeroed K-filter state. A new source follows the restart rule of a discontinuity: momentary,
short-term and their maxima read — until a window lies wholly after its start, and `since`
is its creation time until the first reset.

**Rates.** Loudness runs from 44.1 to 192 kHz when the rate is a whole multiple of 100 Hz
(44.1, 48, 88.2, 96, 176.4, 192 kHz all are), so a 10 ms sub-block is a whole number of
samples and the K-weighting can follow the 48 kHz curve to 19.2 kHz. At any other rate there
are no loudness sources and the pane says *loudness needs a rate of 44.1 to 192 kHz, a
multiple of 100 Hz*.

**Time base.** Each source keeps the K-weighted energy (channel-weighted sum of squares, f64)
of consecutive **10 ms sub-blocks counted in samples**, independent of the audio block size,
in a ring of the last 300.

| quantity | definition |
|---|---|
| momentary (Mom) | the last 40 sub-blocks (400 ms): `−0.691 + 10 log₁₀(mean square)`; evaluated every sub-block |
| short-term (Short) | the last 300 sub-blocks (3 s); evaluated every sub-block |
| max momentary, max short-term | highest values over windows that lie wholly after the reset; −∞ until the first such window |
| integrated (Int) | gating blocks of 40 sub-blocks every 10 sub-blocks (400 ms, 75 % overlap) from the reset; absolute gate −70 LUFS, relative gate −10 LU under the energy mean of the blocks that passed the absolute gate; both gates compare with BS.1770's `>`; re-evaluated at each completed gating block (10 Hz); `None` until a block passes |
| loudness range (LRA) | Tech 3342: the short-term values on the gating grid: the first is the window ending 300 sub-blocks after the reset, then one every 10 sub-blocks (10 Hz); absolute gate −70 LUFS, relative gate −20 LU under the energy mean of those that passed, both compared with Tech 3342's `>=`; P95 − P10; re-evaluated at each new value (10 Hz); `None` until a value passes; *settling* while `duration` < 60 s |
| max true peak | highest true peak of the source's channels since the reset, dBTP (BS.1770 Annex 2, the meters' detector); resets at the same instant as the meters' per-channel max (an audio-block boundary, not the sub-block grid); −∞ until the first metered block |

Momentary and short-term are signal state: they are not reset by `levels.reset`. Each is summed
from the ring at every sub-block (no running add/subtract sum, whose residue can go negative);
a mean square below 1e-15 (−150.7 LUFS) is sent as −∞ and shown —; K-filter states below
1e-30 are flushed to zero so a digital-silence tail does not decay through subnormals. Evaluating them every 10 ms, not every 100 ms, is what
Tech 3341's live max cases need: case 14 starts each 400 ms tone 20 ms further on, and a
100 ms grid would miss up to 50 ms of a tone (−0.58 LU against ±0.1).

**Reset.** `levels.reset { channels }` (and **R** / **Shift+R**) resets every source with a
channel in the set, together with the max and overs: one reset per concept. The reset takes
effect at the first sub-block boundary at or after the first sample of the next audio block
the fan-out thread meters (at most 10 ms later); the gating grid, the LRA feed, the maxima and
`duration` start at that boundary, and the first gating block counts 400 ms later. The
sub-block grid itself never re-phases. The keytip
says *resets max, overs and loudness* (a pair's integrated loudness restarts when either
input's row is reset). Loudness accumulated in a device preview does not carry into the
session: every source starts afresh at `session.open`, as at a daemon restart.

**Capture discontinuity.** When the audio path reports a discontinuity (xrun, device
recovery), the K-filter state and the sub-block ring are cleared and the sub-block grid, the
gating grid and the LRA grid restart at the first resumed sample; partial sub-blocks, gating
blocks and short-term windows from before the gap are discarded. Momentary, short-term and
their maxima use only windows lying wholly after the restart, and read — (−∞) until one
exists. Integrated, LRA and the maxima keep what they had; `duration` counts metered
samples (those of a dropped partial sub-block too; the gap excluded).

**K-weighting at every rate.** BS.1770-5 Annex 1 gives the two biquads for 48 kHz only. The
coefficients for rate `fs` are computed in f64 (and the filter state is f64: at 192 kHz the
RLB poles lie about 1e-3 from the unit circle) by the parameterisation that reproduces Table 1
at 48 kHz to about 1e-15:

- pre-filter: `K = tan(π f₀/fs)`, `Vh = 10^(G/20)`, `Vb = Vh^0.4996667741545416`,
  `a0 = 1 + K/Q + K²`; `b = [Vh + Vb K/Q + K², 2(K² − Vh), Vh − Vb K/Q + K²] / a0`,
  `a = [1, 2(K² − 1)/a0, (1 − K/Q + K²)/a0]`; `f₀ = 1681.974450955533 Hz`,
  `G = 3.999843853973347 dB`, `Q = 0.7071752369554196`.
- RLB high pass: `K = tan(π f₀/fs)`, `a0 = 1 + K/Q + K²`, `b = g·[1, −2, 1]` with
  `g = a0(48 kHz) / a0(fs)`, `a = [1, 2(K² − 1)/a0, (1 − K/Q + K²)/a0]`;
  `f₀ = 38.13547087602444 Hz`, `Q = 0.5003270373238773`. Table 2's numerator is the
  unnormalised `[1, −2, 1]`, which gives the stage a +0.043 dB passband gain at 48 kHz; `g`
  is 1 there and keeps that gain at every other rate (libebur128 keeps `[1, −2, 1]` and so
  drifts by up to 0.04 dB at 192 kHz).

Tests: at 48 kHz the coefficients equal Tables 1 and 2 to 1e-14 (with `g` = 1); at 44.1,
88.2, 96, 176.4 and 192 kHz the
cascade's magnitude response matches the 48 kHz one within ±0.01 dB from 20 Hz to 19.2 kHz;
tone vectors at 40 Hz, 1 kHz and 10 kHz read the same integrated loudness at every rate as at
48 kHz within ±0.05 LU; at 32 kHz (below the range), 47.952 kHz (not a multiple of 100 Hz)
and 384 kHz (above the range) there are no loudness sources and the pane reads *loudness
needs a rate of 44.1 to 192 kHz, a multiple of 100 Hz*.

**Bounded memory.** A show is hours long; neither gate may keep every block. Integrated and
LRA each keep a histogram of 0.01 LU bins from −70 to +10 LUFS (8000 bins; bin `i` covers
`[−70 + 0.01·i, −70 + 0.01·(i + 1))`, index `floor((L + 70)·100)` in f64, except that
integrated sends `L ≤ −70` to underflow; an underflow bin
for what the absolute gate drops and an overflow bin above, f32 input can exceed 0 dBFS)
holding each bin's count and f64 energy sum: 256 kB per source, fixed. Integrated
puts values `≤ −70` in its underflow bin (its gate is `>`), LRA values `< −70` (its gate is
`>=`). The relative threshold Γ comes from the exact energy sums. **Integrated:** bins wholly
above Γ count whole; the bin Γ falls in counts the fraction `(upper edge − Γ) / 0.01` of its
count and energy. When Γ lies below the histogram (integrated Γ ≤ −70, LRA Γ < −70) the
absolute gate governs: every regular bin and the overflow bin count whole, the underflow
bin never counts. Integrated: if Γ is at or above +10 LUFS (every block in the overflow bin) the
overflow bin's blocks count whole and the bound does not apply. **LRA:** the bin Γ falls in counts whole if its lower edge is `>= Γ` (the overflow
bin, when Γ falls in it, counts whole), otherwise not, so the gated count `n` stays an integer; the percentile index is the integer
`k = ((n − 1)·p + 50) / 100` (integer division; zero based; equal to the Tech 3342 MATLAB's
`round` for p = 10 and 95); the value is the centre of the bin where the cumulative count
first exceeds `k`, or the energy mean `−0.691 + 10 log₁₀(E/c)` of the overflow bin if it
lands there. Bound: the result departs from an unbinned computation only through blocks in
the bin a gate falls in (within 0.01 LU of the gate, where the exact value itself jumps for
a 0.01 LU change of the input) and by half a bin (0.005 LU) on each LRA percentile. A refgen
vector with a dense population of blocks around the relative gate checks it: the binned
result lies between the unbinned results with every block of the gate's bin included and
excluded, widened by 0.01 LU for LRA.

**Golden vectors.** `tools/refgen` synthesises the EBU cases from their text and records the
published reading and tolerance; its reference implementation (numpy, unbinned, per block,
the formulas above) gives values for the same signals at the other rates, for a mono source
and for the histogram case:

- Tech 3341 cases 1–2 (momentary, short-term and integrated, ±0.1 LU); 3–5 (integrated,
  ±0.1 LU); 9 (short-term: every sub-block value after 3 s within ±0.1 LU); 12 (momentary:
  every sub-block value after 1 s within ±0.1 LU; its 0.4 s period does not divide the 3 s
  window, so short-term is not asserted); 11 and 14 in their live form (one
  signal, max short-term sampled at the end of each 6 s segment / max momentary at the end
  of each tone segment, the twenty successive maxima, ±0.1 LU), each run twice: with the
  reset on a sub-block boundary at the signal's start and with the reset 5 ms (240 samples
  at 48 kHz) before it, the grid's worst phase;
  15–19 (true peak, +0.2 / −0.4 dB).
- Cases 20–23 (an fs/4 period spliced into an fs/6 tone, made at 4·fs and decimated): the
  documents do not specify the decimator, splice or fade, so refgen states its own (a
  windowed-sinc decimator of named length and cutoff, a phase-continuous splice, a named
  fade) and the vector carries two expectations, both of which must hold: the published 0.0
  +0.2 / −0.4 dBTP, and refgen's own true peak of its signal by a long-FIR ×32
  reconstruction within the same tolerance. They check ac2's
  detector, not EBU equivalence.
- Tech 3342 cases 1–4 (LRA of tone sequences, ±1 LU, read at the end of the signal with no
  trailing silence fed), and case 1 repeated twice (LRA unchanged).
- Quiet programme (mean about −62 LUFS, so the relative gates fall below −70): integrated
  and LRA match the unbinned reference with the absolute gate governing.
- Discontinuity (fake backend, an xrun between two tone segments): integrated carries on
  across the gap, `duration` excludes it, momentary reads −∞ for the first 400 ms after the
  resume and short-term for the first 3 s, and no block spanning the gap is counted.
- The −18 dBFS stereo 1 kHz calibration tone reads −18.0 LUFS.
- Not run, said so in the vector set: 3341 case 6 (5.0, synthesisable but ac2 has no
  surround source), 10 and 13 (the file form of 11 and 14: twenty files each measured after
  a reset; the live form tests the same windows), 7–8 and 3342 5–6 (the EBU's own programme
  recordings, not in the text).
- K-weighting tests run at 44.1, 48, 88.2, 96, 176.4 and 192 kHz.

**Wire.** `SessionLevelsMeta` and `PreviewLevelsMeta` gain `loudness: Vec<SourceLoudness>`,
pairs first in `pair_levels` order, then unpaired inputs ascending by channel; channels in
`LoudnessSource` are zero-based device channels as in `PairLevels` (the CLI and UI show
them 1-based):

```rust
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LoudnessSource { Pair { left: u16, right: u16 }, Input { channel: u16 } }
#[serde(deny_unknown_fields)]
pub struct SourceLoudness {
    pub source: LoudnessSource,
    pub momentary: Lufs,          // −∞ on silence
    pub short_term: Lufs,
    pub integrated: Option<Lufs>, // None until a block passes the absolute gate
    pub range: Option<Db>,        // LU
    pub max_momentary: Lufs,      // −∞ until the first window after the reset
    pub max_short_term: Lufs,
    pub max_true_peak: Dbtp,
    pub duration: Seconds,        // metered samples since the reset: settling, elapsed
    pub since: WallNs,            // the daemon's clock at the reset, for "since 20:14" only
}
```

`Lufs` is a new unit newtype beside `Dbfs` and `Dbtp`; a loudness difference (LRA) is a
`Db`, shown with the unit LU.

`duration` carries what must not depend on clocks (a Pi has no RTC; NTP steps it after
boot): LRA settling is `duration < 60 s`. `PROTO_VERSION` bump. Not persisted: a daemon
restart or a new session starts every source afresh.

**Pane.** A Programme pane shows a loudness column for its pair; for a pane of one input,
that input's mono source, or the pair's loudness (titled with the pair's name) when the input
is half of a pair; for a pane whose inputs are exactly a declared pair, that pair; a pane of
several other inputs shows none and says *loudness: one input or a pair*; a pane on an input
whose pair partner is not captured names the partner as the pair row does, *loudness: Main R (input 4) not captured*. Mic inputs show
*no loudness on a mic input*. When several messages apply, the pane shows the first of: the
rate message, the mic message, the partner not captured, *one input or a pair*. The column reads `Mom −23.1`, `Short −22.8`, `Int −23.0 LUFS`,
`LRA 6.2 LU` (`LRA 6.2 LU (settling)` while `duration` < 60 s; *LRA — (waiting for
programme)* while it is `None`), `TP max −1.2 dBTP`, then
*since 20:14 · R resets* (the SPL meter's wording); integrated before a block passed the gate
reads *Int — (waiting for programme)*. One decimal throughout. (Mom / Short, not M / S: mid
and side are the goniometer's words in the same pane.) Narrow panes drop the column before
the goniometer; **T** hides the since line with the rest of the chrome. The Inputs list rows
stay as they are.

**CLI.** `ac2 levels` gains a loudness table (source · Mom · Short · Int · LRA · max Mom ·
max Short · TP max · since); `--json` carries it; `ac2 levels reset` already resets it. The
table names a source by 1-based inputs as the existing rows do (`1+2`, `3 mono`). In `--json`
a source is `{"type": "pair", "left": 1, "right": 2}` or `{"type": "input", "input": 3}`;
−∞ is the string `"-inf"` (as `spl_log` writes it) and `None` is `null`, so silence and
*waiting for programme* stay distinct. The existing rows' `ppm`, `true_peak` and `max` switch
to the same `"-inf"` encoding in this change, so `null` never stands for −∞; it keeps each field's own meaning (not yet, gated, none).

Not in this stage: a loudness log, session persistence of the integrated value, EBU Mode's
relative readout, bar scales and pause, surround. If a show's integrated loudness turns out to
be wanted after the fact, it becomes a measurement with a log like the SPL meter's; the pane's
numbers stay the live view.

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
- Stage 3: `SessionLevelsMeta` / `PreviewLevelsMeta` gain `loudness: Vec<SourceLoudness>`
  (§6.1), a `Lufs` unit; `PROTO_VERSION` bump; nothing persisted.
- `PaneKind::Programme` and `PaneContent` in `ac2-proto` layout (the session's per-measurement
  layouts, decision P1) and the app's layout prefs.

### As built in stage 2

- `PaneContent` is `Meas { meas } | Inputs { inputs } | Pair { left, right }`: a pair is named
  by its two inputs, which is what the session's pair list keys on; there is no pair id.
  `PaneLayout::check` refuses a Programme pane without inputs and inputs on any other kind.
- A picked Inputs row opens its Programme pane with **P**, not **Enter**: Enter fires the
  stimulus in every keymap (`RESERVED`), and a meter key must never be one chord away from
  sound. A click picks a row; Tab or a click elsewhere drops the pick.
- The goniometer frames come with the input meters (`session/` subscription), not a
  subscription of their own while a Programme pane is visible; the app folds them into ember
  rings only for the pairs a visible pane shows.
- `PairLevels.quiet` names one side, the left first, so the chip never says *both quiet*.
- The rig's four values are on Settings › Rig monitor (←/→ steps, each step one `levels.set`
  of that value) besides `ac2 levels set`.

## 9. What is and isn't claimed

- The ballistics are implemented to the named standards' burst tables and proven by
  tone-burst vectors; the meters are not type-approved instruments and the user guide says so
  once.
- dBu readings rest on a voltmeter and are as good as it; the row shows the calibration's
  age as mic calibrations do.
- Correlation is at zero lag: an inter-channel delay (a misaligned stereo pair) lowers it
  honestly; the meter does not search lags, and the pane does not guess why it is low.
- The goniometer shows shape at auto gain, never level; nothing in it is a number.
- Loudness follows BS.1770-5 and the Tech 3341 / 3342 definitions and passes the stereo
  live-meter cases synthesisable from the documents' text, each on the quantity it specifies (3341: 1–5, 9, 11, 12, 14, 15–19;
  3342: 1–4); 3341 cases 20–23 are checked against ac2's own synthesis of them; 6 (5.0) and
  the file forms 10 and 13 are not run. At rates other than 48 kHz the K-weighting is ac2's
  derivation (BS.1770 gives 48 kHz coefficients and asks for the same response), matching the
  48 kHz response within 0.01 dB from 20 Hz to 19.2 kHz; loudness runs from 44.1 to 192 kHz
  only. Integrated loudness and LRA come from 0.01 LU histograms, within the stated bound of
  an unbinned computation. It is not an EBU Mode meter (no relative readout, no +9 / +18 bar
  scale, no start / pause / continue) and not surround. Momentary is evaluated every 10 ms
  (3341 states no minimum rate for it).
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
