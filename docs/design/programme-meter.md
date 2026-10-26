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
| `ppm` | the chosen ballistic's reading; where VU is chosen but not offered (a mic or digital input, or an uncalibrated line input) the true-peak reading, and the row says *true peak shown · VU needs a line calibration* (the order of §6.2); under a loudness ballistic (§6.2) every input's true peak — a rig setting never blanks a bar |
| `true_peak` | dBTP, this frame |
| `max` | the highest reading of the chosen ballistic since the last reset (daemon; held per ballistic, since all run, so a ballistic change does not splice a Type I maximum onto true-peak readings); under a loudness ballistic (§6.2) the true-peak hold |
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
calibrations) and the rig's settings (ballistic, correlator window, over threshold, and from stage 4 the
loudness target, §6.2). **W** /
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
+18 scale and no start / pause / continue (§9); stage 4 (§6.2) adds loudness bars on the +9
span in LUFS, under the `Momentary` / `ShortTerm` ballistics only, and turns the column below into a plate.

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
are no loudness sources; from stage 4 (§6.2) the reason, *loudness needs a rate of 44.1 to
192 kHz, a multiple of 100 Hz*, is on each bar's reason row under a loudness ballistic
(`true peak shown · loudness needs …`), and under a peak ballistic nothing on the pane
claims loudness and there is no plate.

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
and 384 kHz (above the range) there are no loudness sources: under a loudness ballistic each bar's reason row reads
*true peak shown · loudness needs a rate of 44.1 to 192 kHz, a multiple of 100 Hz*, under a
peak ballistic the pane has no plate and no loudness text (§6.2).

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

`Lufs` is the unit newtype (stage 3) beside `Dbfs` and `Dbtp`; a loudness difference (LRA) is a
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
and side are the goniometer's words in the same pane.) Stage 4 (§6.2) replaces this
column, its messages, its narrow-pane rule and its **T** behaviour: the readouts move to the
loudness plate, the mic / partner / rate reasons onto the bars' reason rows under a loudness
ballistic, and *one input or a pair* goes.

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

### 6.2 Loudness bars and the loudness plate (stage 4)

Stage 3 put loudness in small text lines at the pane's edge. Mastering for a delivery is done
by eye on a bar and one large number, not by reading lines, so stage 4 makes loudness a
ballistic the bars can read and turns the column into a **plate** that moves, resizes and
hides with **T** like the plates over the graphs. Loudness stays on line and digital sources
only (§6.1 *Sources*); nothing here meters a mic.

**What a bar reads.** Under a loudness ballistic a bar is a **source**, not an input: a pair
reads one bar, the BS.1770 loudness of both channels summed with weights 1, 1; an unpaired
line or digital input reads its mono source. There is no per-input loudness bar. BS.1770 and
Tech 3341 define loudness of a programme, one figure for all its channels; the EBU Mode
meter shows one M or S bar per programme. A channel K-weighted alone is not a loudness of
anything delivered: on a centred mono mix each side reads 3 LU under the pair, two such bars
beside a pair's Int read as a 3 LU disagreement that is not there, and equal side bars would
pass for balance, which the peak bars, the correlation and the goniometer already show
honestly. The bars of a pane are therefore its sources, in the order the pane first names
one of their inputs: a Pair pane has one bar; an Inputs pane of `1, 5, 2` with `1+2` a
declared pair has two, *Main* (where input 1 was) and input 5; a pane of one input that is
half of a pair reads the pair's bar under the pair's name, as its loudness does in §6.1, and
so does a pane naming one half among other inputs (`1, 5` with `1+2` declared and input 2
captured but not in the pane: bars *Main* and input 5, the *Main* bar with both sides'
flags). `loudness_column()` today resolves a pair only for one input or exactly two;
`sources()` (below) generalises that resolution to any input list.
Bars are equally wide; a source bar of a pair is not drawn double width.

The scene's model (4c) becomes a list of bars, filled by the UI (4e):

```rust
pub enum ProgrammeBar {
    /// One input's peak bar: every peak ballistic, and an input without a source under a
    /// loudness one, with the reason in `note` (`programme::bars_of()` formats it from
    /// `LoudnessFacts` in §6.1's message order: rate, mic, partner not captured). Under
    /// a peak ballistic `bars_of()` fills it with `input.note(ballistic)` (the VU fallback),
    /// so the pane reads one field; `ProgrammeInput::note()` stays for the Inputs list.
    Peak { input: ProgrammeInput, note: Option<String> },
    /// A loudness source's bar.
    Source(SourceBar),
}
pub struct SourceBar {
    pub name: String,            // `Main`, `Vox mono`
    // f64 throughout, as `Lufs` and `Dbtp` are; the frame's f32 true peak widens.
    pub reading: Option<f64>,    // LUFS of the chosen window; None for −∞
    pub max: Option<f64>,        // the window's max since the reset
    pub true_peak: Option<f64>,  // dBTP, the higher side this frame
    pub sides: Vec<SideFlags>,   // per input, left first: overs, clipped
}
```

with `pub struct SideFlags { pub overs: u32, pub clipped: bool }`. `ProgrammePane` gains
`loudness_target: f64` (LUFS), filled by the UI from the rig's `levels` settings it already
reads for the ballistic (`s.rig.levels` in `ac2-ui/src/state/programme.rs`; −23, the
default, while no rig state has arrived, so a preview has a span), and beside
`bar_fill` stands `lufs_fill(reading, target) -> f32`: 0 at target −18 LU, 1 at target +9 LU,
clamped. `ProgrammePane` keeps `inputs: Vec<ProgrammeInput>` (one per pane input, every
ballistic), which the Inputs list rows (`ProgrammeInput::row_lines` in `ac2-scene/src/programme.rs`, called from
`ac2-ui/src/view/chrome.rs`) and the
preview rows keep reading, and gains `bars: Vec<ProgrammeBar>`, which the pane draws;
`layout()` takes `bars.len()`. A device preview builds its `LoudnessFacts` from the preview frame
(the kinds and pairs `session.preview` declared, `PreviewLevelsMeta.loudness`, which §6.1
already carries), so a preview pane reads loudness bars, target and plate as a session's
does. With no levels frame at all (no session, no preview) `bars` is one `Peak` per input
with the VU note only, and the pane shows its missing text anyway. Two steps,
one resolution:

- `programme::sources(shown: &[u16], facts: &LoudnessFacts) -> Vec<Resolved>` (`enum Resolved { Source(LoudnessSource), Reason(String) }`, 4c's
own), one per pane
  input: its source (`LoudnessSource`) or its reason (rate, mic, partner not captured, in
  §6.1's order). It generalises today's `loudness_column()` resolution to any input list and
  is the only place an input is mapped to a source.
- `programme::bars_of(resolved: &[Resolved], ballistic, facts: &LoudnessFacts) ->
  Vec<ProgrammeBar>`: under a peak
  ballistic one `Peak` per input with `note` = the VU note or `None`, as today (a pair pane keeps its L and R
  bars); under a loudness one, one `Source` bar per distinct source at its first input and
  `Peak { note: Some(reason) }` for an input without one.

A source bar needs its sides' peak data even when the pane does not show a side (the
half-pair case above), so `LoudnessFacts` gains `input: &dyn Fn(u16) -> Option<ProgrammeInput>`,
every captured input's `ProgrammeInput` as the UI builds it today from the levels frame
(`ac2-ui/src/state/programme.rs`), filled as it fills `is_mic`. `bars_of()` takes both a
`Peak` bar's `input` and a source bar's per-side `true_peak_dbtp`, `overs` and `clipped`
from it; there is no separate peak-facts type. The plate and the bars read the same `sources()`, so they never disagree on which
source an input belongs to. One function runs the dock
ladder (below): `programme::dock(rect: Rect, has_pair: bool, bars: usize, scale_w: f32,
font: f32, scale: f32) -> ProgrammeLayout`, trying each step by calling `layout(rect: Rect,
has_pair: bool, bars: usize, scale_w: f32) -> ProgrammeLayout` on the room left after the
reserve; `layout()` stays the pure fit for a given room and loses its `loudness: bool`. The
pane calls `dock` when it has a plate (`loudness.is_some()`) whose place is auto
(`plate.at.is_none()`), at every T step (so the room stays reserved at the third); otherwise
it calls `layout` on the whole rect (a floating plate reserves nothing). `ProgrammeLayout.
loudness: Option<Rect>` becomes `plate: Option<Docked { rect: Rect, dropped: u8, scale: f32
}>`, `dropped` 0 … 4 being how many steps of the fixed drop order (foot, maxima, range and
peak, live) are taken; `loudness_plate::size(font, scale, dropped)`, the drawing and a
floating plate's shrink use the same count. `LOUDNESS_W` goes. The scale width is one number, `scale_w(kind, small)` (the dBFS column `3 · small` as
`draw::bars` reserves today, or the LUFS column below), used by the fit test, the dock and
the drawing alike, replacing `layout()`'s fixed `BAR_SCALE_W`. `ProgrammePane.loudness:
Option<LoudnessColumn>` becomes `Option<LoudnessReadout>` (the plate's data), present iff
`sources()` resolves the pane to exactly one source, whether or not this frame carries that
source yet: a resolved source absent from the frame's `loudness` (declarations and frames
arrive on different paths) reads −∞ / `None` in every field, `duration` 0 and a blank foot,
and a `SourceBar` for it is empty with — readings, so the plate and its dock never pop in a
frame later; `LoudnessColumn` and
`ONE_INPUT_OR_PAIR` go; `LoudnessReadout` gains `max_momentary` and `max_short_term` for the plate's
maxima slot; the plate takes `LoudnessReadout` (so extended) as its data; its sizes come from a new
`programme::loudness_plate::size(font, scale, dropped)
-> (w, h)` returning the size at the slots' mixed font sizes (2.5 f, 1.5 f, f, 0.8 f), which
`crate::plate::Metrics::size` (one row size) cannot: each slot's row is 1.3 × its size, as every
bar row, and the pads are `crate::plate::Metrics::of(font, scale).pad_x` / `pad_y`, so the dock's
fit tests in the scene and the golden images agree.

**Two ballistics, one per window.**

```rust
pub enum Ballistic {
    TruePeak, Type1, Type2, Vu,
    /// BS.1770 momentary loudness of the bar's source (400 ms), LUFS. Line and digital
    /// sources only; an input without a source reads its true peak.
    Momentary,
    /// BS.1770 short-term loudness of the bar's source (3 s), LUFS.
    ShortTerm,
}
```

Two values rather than one *Loudness* ballistic with a window setting: the ballistic already
is the rig's one answer to "what does a bar read", the two windows are the two Tech 3341
live readouts, and a separate window setting would be a second control for one concept.
Words (chooser, title, toast, CLI table header): `Mom loudness`, `Short loudness` (Mom /
Short as in §6.1, never M / S, which are the goniometer's); `unit()` is `LUFS`, used only where the ballistic itself is named
(the chooser, the CLI loudness table's `(bars)` mark), never for an input: a per-input unit
comes only from `ProgrammeInput::unit()` via `reads()`; serde
`momentary`, `short_term`; CLI `--ballistic momentary|short-term`.

The detectors and the choices are two lists: `Ballistic::PEAK: [Self; 4]` (true peak, Type I,
Type II, VU: what each input runs) and `Ballistic::ALL` = `PEAK` then `Momentary`,
`ShortTerm` (what a chooser steps). The daemon's per-input arrays (`interval`, `hold`) and
its `slot()` are sized and indexed by `PEAK`, never `ALL`; `slot(Momentary | ShortTerm)` returns
the true-peak slot, and the VU fallback branch of `take()` becomes `chosen.is_loudness() ||
(chosen == Vu && !vu_offered)`. `Ballistic::is_loudness()` says which. 4a defines
both lists; the mirrors gain the two values in their layers: `ac2_scene::programme::
Ballistic` (4c), the UI's `ballistic()` map in `ac2-ui/src/state/programme.rs` and the
choosers on Settings › Rig monitor and the form, which step `ALL` (4e), the CLI's
`BallisticArg` (4b: a clap `ValueEnum`, so variants `Momentary` / `ShortTerm` give
`momentary` / `short-term`, and its `From` impl gains both arms). Integrated is not a ballistic: it has no window, it is a figure since the
reset, and it is the plate's large number.

**Scale and target.** A loudness bar is drawn on the span of the EBU +9 scale, **target −18
LU … target +9 LU**, labelled in absolute LUFS every 3 LU (target −23: −41, −38, … −23, …
−14; `target −41`, the template of the scale column below, is the widest label over the whole target range) when the bar is at least ten label rows (`1.3 · small` each) high, else every 9 LU
(−41, −32, −23, −14); the target tick always. The rule depends on the bar's height only, so
labels never come and go with values. The glance a bar is for is "how far from target", and 27 LU over a bar's height gives
about twice the resolution per LU of the −60 … 0 span, so 1 LU is visible. Below the
span the bar is empty and its text still reads the value; above it the bar is full. A max
outside the span draws no tick either way (clamped, it would read as *max = target +9* or
*target −18*); the text carries the value. The
numbers stay absolute (LUFS): ac2 still has no relative LU readout (one unit per quantity;
a relative mode would be a second number for the same thing). The target is a new rig
setting, `levels.loudness_target` (LUFS, default −23, the R128 programme level; −36 … −5 in
1 LU steps, which spans R128, ATSC A/85 −24, the −16 … −14 of streaming services and AES
TD1008's −16 … −20). It is the rig's, not the app's, because it colours the bar: two apps on
one rig must not show the same programme lit above the target on one and not on the other.

**Title.** `ProgrammePane::title()` starts with the word of what the bars read, not of the
setting alone: `Mom loudness` / `Short loudness` when any bar is a `Source`, else `true
peak` (a pane of mics or of sourceless inputs under a loudness ballistic, whose bars all read
true peak). The same rule applies to VU: a pane where no bar offers VU is titled `true peak`. The target follows a loudness word: `Short loudness · target −23 LUFS · Main ·
1 s`.

**Lines on a bar.** Two lines, drawn so one never passes for the other: the **target**
(a setting) is dashed, 1 px, in the text colour (the SPL limit line's colour; that line is
solid, and the dash is what tells a target from a reading), and spans the bar and its LUFS
ticks; the **max since the reset** (a reading, below) is a pair of short ticks, 4 px wide and
2 px high, just outside both edges of the bar, in the dim text colour, so it is neither the
target's dash nor the flying peak's solid bar across a peak bar beside it. The target crosses
LUFS bars only; its label is the tick at the target itself, which reads `target −23` in
place of the plain `−23` (one label, on the pane's LUFS scale). At *no grid* that
tick goes back to the plain number; the tick numbers go at *traces only*. The LUFS scale
column is `text_width("target −41", small)` + 4 px wide, a
constant at every T step, so the label fits and nothing moves when it changes to a number.

**Colour.** Zones by position, not by state: the part of the bar below the target is the
level green (`Theme.level_ok`), the part above it that green blended halfway to the text colour (lighter on the dark theme,
darker on the light one; the test asserts the blend). No yellow and no red: on every other bar yellow means *near full scale* or
*NEAR a limit*, and a Mom bar of a programme whose Int sits on target is above the target
half the time, which is not a fault (CLIP and overs stay on the bar's text, coloured as
today). A bar near target grows a lighter tip instead of flickering its whole colour at
every crossing. The target line stays at every **T** step (as the SPL
limit lines do), the label goes with the grid (as the SPL limit text does).

**Holds.** No flying peak on a loudness bar: Tech 3341 defines a maximum since the reset for M
and S and no falling hold, and a hold-and-fall over a 400 ms or 3 s mean would read as a peak, which it is not. Instead the
**max since the reset** (`max_momentary` or `max_short_term` of the source, the daemon's,
already on the wire) is the pair of ticks at the bar's edges, `max −20.1 LUFS` under it; −∞ (no window
after the reset yet) draws no line. **R** clears it with the rest. The bar is fed at the frame rate (*Rate and cost*, below),
which meets Tech 3341's 10 Hz for live readouts.

**The bar's text** (fixed rows, so a bar never grows or loses a row as values change; the
pane's row count is the largest over its bars, set by the ballistic and the inputs' kinds,
never by values, so every bar has one top and one height and the LUFS and dBFS spans share
it): the
reading `−23.1 LUFS` (— before a window lies wholly after the start, a reset does not
affect it), `max −20.1 LUFS`, `TP −1.2 dBTP` (the higher of the source's inputs this frame),
and the flags row: on a mono source as today; on a pair each side's overs and CLIP named
by side, left first, overs before CLIP, joined by ` · `: `1 over L`, `2 overs L · 1 over R`,
`CLIP R`, `2 overs L · CLIP L · CLIP R`; the longest form, which sizes the row's cut, is
`999+ overs L · 999+ overs R · CLIP L · CLIP R` (counts above 999 read `999+`), cut to the
bar's width with an ellipsis as every bar row is. The row is left-aligned at the bar
column's left text edge (`xl`), where overs sit today, so it grows only to the right as flags
come; a mono source bar keeps today's layout (overs at `xl`, `CLIP` at `xr`).
The label over a bar is the source's name: the pair's name, an input's name with *mono*.

**An input without a source** (a mic; an input whose pair partner is not captured; any input
at a rate outside §6.1 *Rates*) reads its **true peak** under a loudness ballistic, by the
VU rule of §3 (a rig setting never blanks a bar), with the reason as its last row: `true peak
shown · ` and then §6.1's own text (`MIC_TEXT`, `partner_text()`, `RATE_TEXT`: *true peak
shown · no loudness on a mic input*, *true peak shown · loudness: Main R (input 4) not
captured*, …), reading first so that a cut to the bar's width (ellipsis, as every bar row)
loses the reason, never what the bar reads. `VU_FALLBACK` (`programme.rs`) flips to the same
order, *true peak shown · VU needs a line calibration*, in 4c, and the user guide's VU
sentence with it: one surface, one order. It is a dBFS bar with its flying peak and
its whole-bar colours as on any peak bar. In a pane whose other bars read LUFS it carries its
own short dBFS scale at its left edge (0, −20, −40, −60), since the pane's scale is LUFS: under a loudness ballistic the pane's
scale (on the left) is LUFS iff any bar is a `Source`, whatever the order, and only peak bars
carry their own ticks (the four dBFS numbers, narrow); a LUFS bar never carries its own. A pane of mics only has the dBFS scale and no LUFS one, and no target line. A scale here
is a span: the LUFS span of a loudness bar, or the 60 dB span under full scale every peak bar
has (a dBu or dB SPL bar shares it; only its numbers differ, as today), so only a LUFS bar
beside a peak bar counts as differing.

Under a loudness ballistic every **per-input** reading is the true peak, and every consumer
of one says so: `ProgrammeInput::reads()` returns `TruePeak` whenever the ballistic is a
loudness one (the VU rule of §3 generalised), so its `unit()` is dBTP and its text has no
note (the note is for an input whose bar would read loudness and cannot, above); the Inputs
list rows and the I/O page's device-preview rows read true peak, in dBTP (dBu / dB SPL on a calibrated scale, as under true peak); the `ac2 levels` input table heads that column `true
peak`: `ballistic_of()` in `ac2-cli/src/cmd/levels.rs` returns `TruePeak` whenever the
rig's ballistic `is_loudness()` (the VU rule generalised, as `reads()`), so the table's word
and unit and the per-input `--json` `"ballistic"` (`"true_peak"`) follow (4b, with a test) and its loudness table marks the bars' column (`Short (bars)`); `--json` numbers are
unchanged (`ppm` is the true peak).

**Rate and cost.** Nothing new is computed: the fan-out thread already runs every source's
meter and evaluates momentary and short-term at every 10 ms sub-block (§6.1), and the levels
frame (30 Hz) already carries their latest values in `SourceLoudness`; a bar draws the
latest value of each frame, up to about 43 ms stale (a 33 ms frame plus the 10 ms sub-block) on a 400 ms window. Tech
3341 asks for at least 10 Hz on the live readouts. The audio callback is untouched; the
fan-out thread does no new allocation per block.

**Wire.** `Ballistic` gains the two values (`levels.set`, `SessionLevelsMeta.ballistic`,
the rig file). Under a loudness ballistic each input's `ppm` and `max` carry its true peak and
true-peak hold (`ppm` is per input; loudness is per source and is in `loudness`), so a frame
stays self-describing: `meta.ballistic` says the bars read `loudness`, the columns say what
every input's own peak is. `LevelsSettings` gains `loudness_target: Lufs`, checked against
`LOUDNESS_TARGET_RANGE` (−36, −5) by `check` in the over threshold's format, raw numbers (`loudness target -40 LUFS is outside -36 …
-5 LUFS`; both bounds negative, so no `+` as the over threshold's upper bound has); `Command::LevelsSet` gains `loudness_target: Option<Lufs>` (sent only when
changed, as the other four); Settings › Rig
monitor and the *How it reads* rows of the New programme meter form gain *Loudness target*
(←/→ 1 LU, one `levels.set` per step, like the other four); the row's words live
in `programme::rig` beside `BALLISTIC_TITLE` as `TARGET_TITLE` (*Loudness target*),
`TARGET_NOTE`, `TARGET_STEP_LU` (1) and `target_text()` (`−23 LUFS`); `ac2 levels set
--loudness-target -23lufs` (a `LoudnessTargetArg` parsed as `OverThresholdArg` is, with
`allow_hyphen_values` and the unit suffix required: `expected a loudness in LUFS (e.g.
-23lufs)`). The pane's layout gains the plate (below). `PROTO_VERSION` 68 → 69;
rig settings file version 5: a new `rig/v4.rs::upgrade` adds `loudness_target` (−23) to
the `levels` section when it lacks one (a version 1 file has had the whole default section inserted by `v2::upgrade`, which
already holds the target; a version 2, 3 or 4 file has its own `levels` without it; the 4b
test reads a version 1 file without `levels` and a version 2 file with one; `levels` denies unknown fields and requires every
field, so the upgrade is needed, not optional) and the loudness plate to every meter layout
(below), following `rig/v3.rs` (which added the peak plate); `file.rs::read` appends it to
every chain (version 1 → v1, v2, v3, v4; 2 → v2, v3, v4; 3 → v3, v4; 4 → v4). Session
format 33 → 34 for the layout (below).

**The loudness plate.** The column becomes a plate, `PlateKind::Loudness` (named *loudness readout*
in hints: "drag: move the loudness readout"; the user guide's plate list says the same), with the plate rules of the graphs (user guide *Moving the plates and their text
size*): drag moves it, the corner grip scales its text 0.75× … 3×, a double-click puts it
back where the pane places it at the theme's size, a move or a size is one **Ctrl+Z** step,
hover makes it opaque with the gesture tooltip. The rule is by source count only: the pane has a plate
iff `sources()` resolves its inputs to exactly one distinct source, whatever the ballistic
(a pair pane under true peak has its two peak bars and the plate), and sourceless inputs
beside it keep their own reason rows under a loudness ballistic; otherwise nothing is docked and nothing reserved. There is
no message line: an input without a source says why on its own bar (above). Contents, in
fixed slots, top down (`f` the plate's font size, `theme.small_font_size` × its scale, as
`plate::Metrics::new` sizes every plate, so a loudness plate at 1× reads like a legend):

| slot | text | size |
|---|---|---|
| title | the source's name (`Main`, `Vox mono`) | f |
| hero | `Int` caption, the integrated value, `LUFS` | value 2.5 f, caption and unit f |
| hero status | `waiting for programme` (a new `WAITING_STATUS`; the value cell shows its own —, unlike §6.1's `WAITING`, which is both) while integrated is `None`, else blank | 0.8 f |
| live | `Mom −23.1` · `Short −22.8`, each with `LUFS` | 1.5 f |
| maxima | `max Mom −18.0` · `max Short −20.1` | f |
| range, peak | `LRA 6.2 LU` with its status cell (`RANGE_WAITING` = `waiting` while the range is `None`, else `settling` while `duration` < 60 s, else blank; `settling`, the longest, sizes the cell) · `TP max −1.2 dBTP` | f |
| foot | `since 20:14 · R resets` | 0.8 f |

Int is the large number because it is the delivery figure and the one thing a bar cannot
show (it has no window); the bar already carries the live window, so Mom and Short are the
second tier, in the order every other surface (§6.1's column, the CLI table, the chooser)
gives them. One decimal throughout; − and — as in
§6.1.

**Static.** Nothing on the plate moves when a value changes. Every value is right-aligned to
a fixed x in a cell sized from the template `−188.8` at its slot's size, every status cell
from its longest word; captions and units sit at fixed x either side; a slot keeps its height
when its text is blank. The plate's size depends only on the font, the scale and which slots
fit (below), never on values: the title is cut (`cut_to`) to the width the value cells set,
so a long pair name never widens the plate. Digits are tabular: the scene's `Label` gains `tabular: bool` (false by default,
set by a `Label::tabular(self) -> Self` builder so only the plate and bar labels change),
set by 4c on the plate's values and on the bars' readings; `ac2-plot` (4d) draws such a label
with the font's tabular figures (Inter's `tnum`, through cosmic-text's `Attrs::font_features`,
which the pinned glyphon 0.12 / cosmic-text 0.19 have); one code path, no fallback. The
sign characters need not be tabular: values are right-aligned, so only the digits must be. The scene sizes the cells with `text_width` of the template, whose fixed
advance per character already assumes equal widths. Integrated is never below −70
(its gate) and the momentary floor is −150.7, so the template covers every value; — sits in
the value cell like a number.

**Where it sits.** Placement `at: None` (where the pane puts it) **docks** the plate: the
pane reserves room for it and lays bars, goniometer and chip out in the rest, so the plate
covers nothing. The dock tries, at the plate's scale, then at 1× (drawing only: the stored `scale` is
kept, so a larger pane brings the operator's size back):

1. **beside**, at the pane's right edge from the top, when the plate's height at that scale
   fits the pane's and the rest keeps the bars their width (`BAR_MIN_W` each) and a pair its
   goniometer or chip (stage 3's rule);
2. **above**, at the top right, reserving its height across the pane, when what is left
   below is at least `BAR_MIN_H` = 160 px high (a bar of 60 px plus up to five text rows and
   the label); the rest below is laid out by `layout()`'s own rules (a pair's goniometer
   under `SHORT_PANE_H` becomes the chip);
3. with fewer slots: the foot, then maxima, then range and peak, then live drop, in that
   order, at 1×, retrying 1 and 2 each time; title, hero and hero status never drop;
4. none: no plate (the bars still read loudness).

A plate the operator has **moved** (`at: Some`) floats: it is drawn over the pane (bars,
goniometer and all) at its fraction of the room the
pane leaves beside it, and the dock's room goes back to the bars and goniometer as soon as
it moves. That re-layout is the operator's gesture, not a value change, so the static rule
is not broken; a double-click docks it again (`at: None`) and resets its scale, as
`PlateMsg::Reset` does for every plate. A docked plate covers nothing, so a see-through fill shows only the pane's background. A floating plate
that does not fit shrinks by `plate::fitted` and then drops slots by the order of step 3.
Which slots show is decided by the pane's size and the plate's scale only, never by values.

**Drawing.** The plate is drawn as every plate: its fill by `legend::plate` (through
`plate::draw_lines`' first step), so it honours Settings › Display › *Plate background*
(see-through / solid, the theme's `plate_alpha`) and the hover opacity like the others, and
its grip by `plate::grip`. Its rows have mixed sizes and fixed cells, which `draw_lines`
(one row size, one alignment) does not lay out, so the plate calls `legend::plate` and
`plate::grip` itself and places its cells; it adds no fill or alpha of its own.

**Scene and UI contract.** As on the transfer and spectrum panes: `ProgrammePane` gains
`plate: PlatePlace` (from the pane's `Plates.loudness`) and the hover state; `ProgrammeScene`
gains a `plates() -> Vec<PlateBox>` method, as `TfScene` and `SpectrumScene` have, holding the loudness plate's box when it is drawn and nothing
when it is hidden by **T** or not shown, so a plate that is not
on screen cannot be grabbed; the UI's `scenes::programme` returns `Option<ProgrammeScene>` (today `Option<Scene>`; it stays an `Option`) as
`scenes::transfer` returns its scene, and the Programme arm of `view/panes.rs` sets the
pane's plates from `plates()` as the other arms do;
the loudness plate's `PlateBox` is built with `area` = the
pane's rect (docked or not), and a floating plate's `at` is a fraction of the pane's rect
less the plate (not of the bars' rect), so `position_for` and the drawing use one room and
the first drag off the dock lands under the pointer; and the
existing `PlateMsg` path (hover, `Move`, `Scale`, `Reset`, `End`) applies to
`PlateKind::Loudness` unchanged. `Plates` (scene) gains `loudness`.

**Persistence.** `LayoutPlates` gains `loudness: LayoutPlate`, required like its siblings in
`LayoutPlates` (the table as a whole keeps its `#[serde(default)]` on `LayoutPane`), so
the plate's place and size travel with the pane as the other plates do: in a measurement's
layout (the session) or in `ui.toml` with the window's panes, and in a rig meter's layout.
The slot sits beside `legend`, `readout`, `mark` and `peaks` (the spectrum's peak-list
plate). Programme panes have no legend, cursor values, mark or peak list; those stay at their
defaults there. Old layouts are mapped, not refused, by the existing `ac2_traces::old_layout`
mechanism: a new `plates_add_loudness(v)` beside `plates_add_peaks`, the same walk (every
pane of the Split / Pane tree, a pane being an object with `kind` and `chrome`; a pane with
no `plates` key keeps none, since `LayoutPane.plates` defaults whole, and inserting a table
holding only `loudness` would fail on the required siblings), adding `loudness = {at:
null, scale: 1}` where it is missing; the module's header list gains *session format 33 and
rig settings version 4*. Its callers:

- **sessions:** `ac2_traces::session::VERSION` 33 → 34 (4a). As at every format bump the
  daemon sets an older autosave aside as `autosave.v33` and starts empty, and `ac2 session
  salvage` brings it back; salvage's layout read (`ac2-cli/src/cmd/salvage/read.rs`) calls `plates_add_loudness`
  after `plates_add_peaks` (a pre-33 tree lacks `peaks` too; every caller keeps that order), so a salvaged pane keeps its layout with
  the plate docked;
- **the rig file:** `rig/v4.rs::upgrade` calls it on each `meters[].layout`, as `rig/v3.rs`
  calls `plates_add_peaks` (the rig reads its meters one by one and leaves out one it cannot
  read, so without this every rig meter would silently vanish). The 4b upgrade test covers a
  pane with and one without `plates`; the app's `ui.toml` (`PanePlatesFile`
in `ac2-ui/src/prefs/file.rs`) gains the key, and with it the scene ↔ wire conversions
`wire_plates` / `ui_plates` (`ac2-ui/src/state/panes/plates.rs`), `PlateKind::ALL` (four
to five, which drives `Plates::is_auto()`; the hover hint's *loudness readout* comes from
`PlateKind::name()`) and `Plates::get` / `get_mut`. `LayoutPlates::all()` gains the fifth entry, so `PaneLayout::check` checks it.

**T.** On a Programme pane: full → no grid → traces only → full, as on every pane. *No
grid* takes the bars' grid lines and the target label; *traces only* also takes the scales'
numbers and the **loudness plate** (whole, wherever it sits: graphs hide their cursor-values
plate at the same step). The bars, their text, the
target line, the max ticks, the goniometer and the chip stay at every step, and **nothing
moves**: a docked plate's room stays reserved while it is hidden, and the scale keeps its width
with its numbers gone, as the SPL scale keeps its room (a change: today `draw::bars` drops the
scale's width at *traces only*, so the bars widen). Toasts name what is shown, as the SPL ones do: `Programme: grid, target label, scale and
loudness plate`, `Programme: no grid or target label`, `Programme: no grid, target label,
scale or loudness plate`
(a `programme_name` beside `spl_name`; like the SPL toast it names the full step's parts
on a pane that lacks one, a pane without a plate included). These three strings are the
Programme pane's step names everywhere they are shown (toast, help); the user guide's T
paragraph quotes them as it quotes the SPL ones. *Traces only* in this section is only the
design's name for the third step (`PlotChrome::Bare`), never shown on a Programme pane.

**Several sources, mics, no source.** By the rule above, a pane with several sources, or
several inputs and none with a source (two mics), has no plate and no message (step 4 of
the dock: nothing reserved): its bars already read each source, and stage 3's *one input or
a pair* would read as "no loudness here" beside two LUFS bars; the plate's absence is the
rule, written in the user guide. A pane of a line input and a mic has one source and so the
plate. A pane whose single input has no source docks nothing: under a loudness ballistic
its bar's reason row says why; under a peak ballistic nothing on it claims loudness, and the
user guide says which inputs have none.

**Reset and settling.** **R** / **Shift+R** as in §6.1. After a reset the plate reads Int —
with *waiting for programme*, max Short and max Mom —, LRA — with *waiting*, TP max —, and
the since line the new time, all in their slots; Short and Mom carry on (signal state). A
new source (a pair declared, a session opened) reads — in Short and Mom until a window lies
wholly after its start (400 ms, 3 s), and its bar is empty until then.

**The form.** The New programme meter form's rows *Bar 1* … *Bar 8* become *Input 1* …
*Input 8* (`programme/form.rs`, the user guide's *Programme pane* bullet): they name inputs,
and under a loudness ballistic the pane's bars follow from them by source. The form's other
words follow (`FIRST_BAR_HINT`, *by name; mics show levels, no loudness*, stays): `PAIR_HINT` drops "bars L and R" (*L and R, goniometer, correlation,
loudness*), `INPUTS_HINT` *one row per input, up to 8*, `CHOOSE_FIRST` *choose input 1*,
`BARS_DISTINCT` *each row names another input, 8 at most*, `NO_BAR` / `MORE_BAR_HINT` in input terms,
and the guide's "A bar never meters an input another bar does" becomes "An input is named
once".

**Keys.** None new: the ballistic and target are settings rows (Settings › Rig monitor, the
form), the plate takes the plates' mouse, **R** and **T** are the pane's. The user guide's
*Programme pane* section, *Moving the plates* (its opening "(transfer and spectrum panes; …)" adds the Programme
pane, and the plate list gains the loudness readout) and
*Grid, labels and cursor (T)* (the Programme steps), and the `ac2 levels` row
(the two ballistics, `--loudness-target`) change with the stage, and so do the user guide's Loudness bullet ("a column at the pane's
right edge", "no loudness bar", the messages) and its T sentence, and the *Input meters* and
*Audio I/O* preview sentences (under a loudness ballistic the rows read true peak, dBTP).

**Not in this stage:** a plate per source on a multi-source pane, an Int bar, a relative
(LU) readout, EBU Mode's +18 scale and start / pause / continue, a loudness log.

**Decided here, to confirm with the operator:** no Int bar (Int has no window: a bar of
it would creep and never fall, its number is the plate's hero; adding one later is one more
`Ballistic` value and a PROTO bump, nothing here blocks it); the target defaults to −23 LUFS
(R128, the standard the meter follows) and a rig mastering for streaming sets −14 or −16 once.

**Open for the operator:** whether a multi-source pane should get a plate per source, or a
table plate, later.

**Tests and acceptance.**

- *Values (daemon, fake backend):* no new refgen vectors; the bars show the meter's own
  momentary and short-term, which §6.1's Tech 3341 vectors prove. The stage proves the path:
  3341 cases 1 and 2 played through the fake backend as a line pair and as one line input:
  every frame's `momentary` after 400 ms and `short_term` after 3 s within ±0.1 LU of −23.0
  / −33.0 (pair) and −26.0 / −36.0 (mono, one side of the signal); case 9: every frame's
  short-term after 3 s within ±0.1 LU; case 12: every frame's momentary after 1 s within
  ±0.1 LU; case 14 (live): the bar's max ticks equal `max_momentary` at each segment end.
  Under `Momentary` and `ShortTerm` every input's `ppm` and `max` equal its true peak and
  true-peak hold, a mic's included.
- *Proto:* the two values' serde names, `LevelsSettings::check` on the target range,
  `WIRE_LOCK`, the rig file v4 → v5 upgrade (defaults the target, adds the plate, keeps the rest), `plates_add_loudness` on a tree with and without `plates`, salvage of a format 33 session with a moved peak plate, the session
  format round trip with a moved loudness plate.
- *Scene text:* `reads()` is `TruePeak` under both loudness ballistics, with unit dBTP on
  the bar, the Inputs list row and the preview row; bars per source for a Pair pane, an Inputs pane `1, 5, 2` with `1+2` paired, a pair at 32 kHz under true peak (no plate,
no loudness text) and under `ShortTerm` (true-peak bars with the rate reason row),
  a half of a pair alone, a mic, a partner not captured, an unsupported rate; scale labels at
  targets −23 and −14; fill at target is 18/27; zone split at the target; no flying peak on a
  loudness bar, max ticks at `max_short_term`, none at −∞; the plate's slots for settled,
  waiting, settling and just-reset readings; **static**: the plate's rect and every cell's
  anchor are identical for readings −∞, −150.7, −70.0, −5.0 and +3.2 and for waiting vs
  settled; the dock ladder at pane sizes that land on each step; a moved plate floats and the
  bars reclaim the room; T: the plate absent at *traces only*, the target label
  absent from *no grid*, every bar rect identical across the three steps.
- *Golden images (`ac2-plot`):* a pair pane on `ShortTerm` with the plate docked beside at
  1× and at 2×, a narrow pane with the plate docked above, a value cell at 3× not overlapping its caption, a line + mic Inputs pane (the
  mic's own dBFS ticks) at *full* and at *no grid* (the target label gone, the line kept),
  the *traces only* step.
- *UI:* plate move / scale / reset on the loudness plate store `plates.loudness`, undo is one
  step, the layout persists in the session and `ui.toml`; the T toasts; the target row sends
  one `levels.set`; e2e from an empty daemon: declare a line pair, set `ShortTerm`, open the
  pair's pane, read the scene text.

## 7. Where the settings live

- **Rig settings** (daemon, `levels.set`, beside the system max level; `ac2 levels`):
  ballistic, correlator window (s), over threshold (dBTP), correlation gate (dBFS), and
  from stage 4 the loudness target (LUFS, §6.2: it colours the loudness bars). The
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
- Stage 4: `Ballistic::Momentary` / `ShortTerm`, `LevelsSettings.loudness_target`,
  `LayoutPlates.loudness` (§6.2); `PROTO_VERSION` 69, rig settings file version 5 (`rig/v4.rs`
  adds the default target and the loudness plate), session format 34 (salvage maps older
  layouts through `old_layout::plates_add_loudness`).
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
  an unbinned computation. It is not an EBU Mode meter (no relative readout, no +18 bar
  scale, no start / pause / continue; the loudness bars of §6.2 borrow the +9 span, labelled
  in LUFS; their colour zones, target line and absolute labels are ac2's, Tech 3341
  prescribes none of them) and not surround. Momentary and short-term are evaluated
  every 10 ms (3341 asks for at least 10 Hz on the live readouts).
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
3. **Loudness** (landed) — `ac2_core::loudness` with Tech 3341/3342 vectors, the pane's column,
   `since … R resets`.
4. **Loudness bars and plate** (§6.2, landed) — layers, each one agent:
   - *4a proto + formats* (first; the others build on it; touches `ac2-proto`, `ac2-traces`
     `session.rs` / `old_layout.rs` and `ac2-cli/src/cmd/salvage/read.rs`): `Ballistic::Momentary` / `ShortTerm` with
     words and unit (the doc comments of `Ballistic::unit()`, `SessionLevelsMeta::ballistic`,
     `LevelsSettings.ballistic`, `Command::LevelsSet.ballistic` and `Ballistic::ALL`'s own,
which now says *what a chooser steps* beside `PEAK`'s *the detectors*, say that under `momentary` / `short_term` the per-input
     `ppm` / `max` are true peak in dBTP and loudness is in `loudness`), `LevelsSettings.loudness_target` and its range, `LayoutPlates.loudness`,
     `Command::LevelsSet.loudness_target`, the `LayoutPlates` doc comment (five plates), `PROTO_VERSION` 69, `WIRE_LOCK`, the
     `tools/protocol` cross-language fixtures (the enum's new serde names), session format 34, `old_layout::plates_add_loudness`
     and its salvage call.
   - *4b daemon + CLI* (after 4a): the per-input `interval` / `hold` arrays and their
     comments (`[Ballistic::PEAK]` order), loudness ballistics map `ppm` / `max` to the true-peak slot,
     `levels.set` checks the target, rig file v5 with `rig/v4.rs` (target, and
     `plates_add_loudness` on meter layouts), `ac2 levels set
     --ballistic momentary|short-term --loudness-target`, the fake-backend path tests.
   - *4c scene* (after 4a, beside 4b): `sources()`, `bars_of()`, `dock()` (bars per source under a loudness ballistic, `LoudnessFacts.input`), the LUFS scale, zones, target and
     max ticks, the true-peak fallback bar with its own ticks, `PlateKind::Loudness`, the plate
     slots, the dock ladder, `programme_name`, the scene-text tests.
   - *4d plot* (after 4c): `Label::tabular` drawn with tabular figures, golden images.
   - *4e ui + docs* (after 4c, beside 4d): plate messages for the loudness plate, persistence
     (`PanePlatesFile` in `ac2-ui/src/prefs/file.rs`, `wire_plates` / `ui_plates`),
     the target row on Settings › Rig monitor and the form, T toasts, the user guide sections.
   No `ac2-core` change: the meter already computes every value.

Each stage is one landed batch.
