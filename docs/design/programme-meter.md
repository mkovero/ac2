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
most one pair; both inputs must be of the same kind, mics included (revised 2026-10-26,
operator: *"mic pair can be stereo too"*: an XY or ORTF pair is a stereo source; a mic and
a line input are still refused). A mic pair shows its bars, correlation and goniometer and
no loudness (§6.3 *Mic pairs*); what its correlation means depends on the technique, and
ac2 draws no verdict from it. The pair is named by its inputs' names (*Main L + Main R*) until **N** names it on
the Audio & I/O page.

The Inputs list shows one row per pair: both bars, the correlation chip (§4), each side's overs and CLIP (§6.3 *The Inputs list rows*), and the input numbers when the names lack them. A pair row in the list
selects like any child row (list click model); the Programme panes (§6, §6.3) are where it opens
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
a ring of the points of the last five decays (`GONE_DECAYS`) with their age (§6.3; stage 2 kept a fixed
~1 s, which cut the longer fades short); the renderer places them.

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

Stage 5 (§6.3) replaces this one-pane layout by two views, Meters and Goniometer, stepped
by **G**, and a pair brings one pane of each.

Opened from the picker like any pane, or from a pair row with **P** (stage 5 brings two
panes for a pair, §6.3 *Where panes come from*). Saved in the layout prefs and the session's panes
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
(`plate.at.is_none()`), at every T step (so the room stays reserved at the third; superseded by §6.3: at *traces
only* the room goes to the bars); otherwise
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
moves**: a docked plate's room stays reserved while it is hidden (superseded by §6.3: the
Meters view gives it to the bars), and the scale keeps its width
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

### 6.3 Meter and goniometer as pane views (stage 5)

Stage 4 put bars, goniometer, correlation chip and loudness plate in one Programme pane. In
the field (2026-10-26, ketunkolo, local daemon, one digital stereo pair, `ShortTerm`, target
−22 LUFS) that pane showed a goniometer about 670 px wide on the left, a wide empty gap, one
thin LUFS bar, no loudness plate, and a bar that read empty because −56.5 LUFS lies below
its floor (target −18 LU = −40 LUFS). Stage 5 splits the pane into two **views** stepped by
**G**, sizes the plate from its room, marks a reading below the floor and tidies the Inputs
list's rows (one quantity per bar, with its unit beside it).

**Settled by the operator (2026-10-26):**

1. *"I would treat goniometer and bar metering as their own panes (g-traversible for
   example) and I would expect goniometer to fill the area it is in. with it being its own
   pane i could merely just fullscreen it if I would want to focus on that."*
2. Under a loudness ballistic a pair's pane shows **L and R true peak plus the LUFS bar**
   (MSD / RTW style); a mono line or digital source its true-peak bar plus its LUFS bar.
3. *"option to alter the fade on goniometer, too"*.
4. *"mic pair can be stereo too"*: two mic inputs may be a pair (XY, ORTF); see §2 and
   *Mic pairs* below.

#### Why the field pane had no plate

The scene does dock a plate for that pane. A pair pane under `ShortTerm` with one LUFS bar,
laid out by `ProgrammePane::layout` (`ac2-scene/src/programme.rs`) at 1400 × 760,
1700 × 900 and 2500 × 1300, gets `dock()`'s first rung (*beside*) every time: a plate of
292 × 125 px at the pane's top-right corner (checked with a scratch test against
`programme/tests/bars.rs::Rig`). `plate_readout()` is `Some` when the pane's inputs resolve to exactly one
source, which a line or digital pair pane's always do, and `bars_of()` reads the same `sources()` list. So the plate was laid out and then
not drawn. The code has one path that does exactly that:

- `programme_scene()` in `ac2-scene/src/programme/draw.rs` draws the plate only under
  `style.chrome != PlotChrome::Bare`. `ProgrammePane::layout` still docks at *traces only*
  (§6.2 *T*: "a docked plate's room stays reserved while it is hidden"). The bars therefore
  keep the rest of the pane, the plate's 292 px stay empty at the right, and the scale
  numbers and the goniometer's grid and L/R labels go too. That is the pane the report
  describes: a goniometer, a gap, one bar, no plate, mostly empty. The step is per pane
  (`PaneModes.chrome`) and `ui.toml` keeps it (`chrome = "traces_only"`), so one **T** press on
  an earlier day is enough.

One thing makes it worse even when the plate shows: at 1× in a 1700 px pane the plate is a
292 × 125 box in a corner, far from the bar. The *Meters view* below fixes that by sizing
the plate from its room. The *T* rule stays as it is (plates go at *traces only* on every
pane kind). What changes is that the step can no longer pass unnoticed: a hidden docked
plate now gives its room to the bars (below), and the T toast already names the plate.
Whether the field pane was at *traces only* is still to be confirmed (open question 1).

#### Two views, G steps them

`PaneKind::Programme` gets views as Transfer, Spectrum, SPL and Sweep panes have them:

```rust
pub enum ProgrammeMode { Meters, Goniometer }   // ac2_scene::view, beside SplMode
```

- **Meters**: the bars and the loudness plate (below), no goniometer. A pane naming a pair
  keeps the correlation chip as a strip under the bars, `CHIP_H` high and as wide as the
  bars (operator 2026-10-26: as the MSD does, so a maximised Meters pane still shows it);
  a pane with no pair has no strip.
- **Goniometer**: the goniometer fills the pane, `gonio::square` of the pane less the chip
  strip, centred, with the correlation chip right under the square, centred on it, as wide
  as the square or as its text, whichever is wider (the chip's minimum is its text width,
  not a constant).
  The Goniometer view has **no loudness plate**: `ProgrammePane::layout` does not call
  `dock()` for it, `ProgrammeLayout.plate` is `None`, `programme_scene` builds no
  `PlateBox`, and `plates()` returns none, so no hidden grip or drag target sits on the
  square (today a pair pane docks at every step, `programme.rs` `layout`, and builds a box
  at `chrome != Bare`, `draw.rs`). The plate's place and scale stay stored on the pane,
  for the Meters view. 5b test: a Goniometer-view pair pane on `ShortTerm` has no plate
  box and its square is the pane less the chip strip.
  Below `GONIO_MIN_W` (`programme/layout.rs`, 120 px) of square side, the chip stands alone, centred (5b rewrites
  that constant's comment for this meaning: the smallest square in which a figure still
  reads as a shape). The same
  §5 embers, grid and **T** rule apply: the grid goes at *no grid*, the L/R labels at *traces only*, the chip
  stays at every step.

`ProgrammeMode::next(has_pair)` / `prev(has_pair)` step Meters ↔ Goniometer when the pane
meters a pair, and return `self` otherwise, so a stored value is never overwritten. On a
pane without a pair, **G** toasts `Programme: the goniometer needs a pair` (§2's word) and
changes nothing. `step_view` (`ac2-ui/src/state/queries/select.rs`) loses its
`(PaneKind::Programme, _) => {}` arm. It steps `m.programme` with the focused pane's
`has_pair` (read like `aligned` above it, before `self.modes_mut()` borrows), and raises that toast itself when `has_pair` is
false. `step_view` has no toast path today, so this is its first. A stored `Goniometer` on
a pane whose content stops being a pair (the Edit form picked inputs) draws the Meters view
and keeps the stored value. That is a draw-side fallback, as the transfer scene draws
magnitude for a stored `TransferView::Alignment` (`ac2-ui/src/state/panes/views.rs`)
without an alignment. `PaneModes` gains
`programme: ProgrammeMode` (default `Meters`). Like every sibling view there are three
enums with their mappings: the scene/UI mode `ac2_scene::view::ProgrammeMode` (as
`SplMode`), the wire `ac2_proto::model::layout::ProgrammeView` (as `SplView`; ac2-proto
cannot use a scene type), and the `ui.toml` `prefs/file.rs::ProgrammeViewFile` (as
`SplViewFile`). The mappings are `wire_pane` / `ui_view` in
`state/panes/meas_layout.rs` and `PaneEntryFile::of` / `parse` in `prefs/layout.rs`. Undo already treats `NextView` / `PrevView`
as `PaneEdit::View`.

**No combined view.** The operator wants separate panes. **W** maximises one and **F** makes
it the stage view. A combined view would be a third layout to keep, the stage-4 one whose
1:1 split produced the gap.

**Titles.** Meters: what the bars read, the target after a loudness word, the source or
pair: `Short loudness · target −23 LUFS · Main`. The correlator window stays on a pair's
Meters title, which carries the chip. Goniometer: `goniometer · Main · 1 s` (the window belongs to the
chip). `ProgrammePane::title()` takes the view.

**Where panes come from.**

- **A pair** opened from its Inputs row (**P**, `open_programme` in
  `ac2-ui/src/state/programme.rs`) or from the New programme meter form brings **two
  panes**: the focused pane is split once (`split_focused_pane`, as today) and the new
  pane is split again side by side (`split_pane_at(new, Axis::Row, Side::A)` in
  `state/panes/ops.rs`, which halves the pane, the copy on the `A` side), goniometer left and meters right (the MSD order). The
  meters pane gets the focus. `Layout::split_pane` focuses the new pane, and with the
  goniometer as the new `Side::A` pane the focus would land there, so `open_programme`
  calls `focus_pane(meters)` after the second split. `split_pane_at` fills `meas` from `pane_meas` (`v.meas.or(shown)`),
  and today `open_programme` clears it on the new pane only. After the second split it
  must clear `meas` on **both** Programme panes. The wire is safe either way (`wire_pane`
  takes `content` from `meters` first), but `pane_meas` / `resolve_meas` would otherwise
  tie the goniometer pane to a measurement. 5c asserts neither keeps a `meas`.
  Today **P** is not undoable: `edit_of` in `state/undo.rs`
  has no arm for `C::ProgrammePane`, and the form's submit, which `undo_watch` does watch
  as `Watch::Command` (every `Msg::Key`), reaches `edit_of` with `ran: None` and
  `menu: None` and records nothing. 5c adds `C::ProgrammePane => PaneEdit::Split(was)` to `edit_of`. The form's Enter is a
  `Msg::Key` that runs no `CommandId` (it goes `state/commands/dialogs.rs` → `submit_meter`), so 5c also
  decides, inside the `Msg::Key(_)` arm of `undo_watch` and before it falls back to
  `Watch::Command`, on a new `Watch::MeterForm` (overlay `Overlay::Form` with
  `FormKind::Programme | ProgrammeEdit` and the submit key), which `edit_of` maps to
  `PaneEdit::Split(was)` for New and to `pick_edit(p)` for Edit (its submit runs
  `pane_pick`, as a pane-list pick does). With both, one
  **Ctrl+Z** removes both panes. If there is room for only one split, the meters
  pane opens alone and a toast says `no room for the goniometer pane: G turns this pane to
  it`.
- **Inputs side by side** (no pair): one pane, Meters.
- **A pane's own list** (`PaneMenuRow::Meter`): the pane turns into one Programme pane in
  Meters view: the pick sets `programme` to `Meters`, so a pane whose mode was left at
  Goniometer by an earlier G does not open on the square. The list turns *this* pane into something and never adds panes.
  The reset lives in the `Msg::PanePick` arm of `state.rs`, before it calls `pane_pick`,
  not in `pane_pick`'s `Meter` arm, because the Edit form's submit also runs `pane_pick`
  (`state/meter_form.rs`).
- **The Edit form** applies to its own pane only and keeps its view: editing a Goniometer
  pane's pair leaves it a Goniometer pane (a 5c test), and if the edit leaves it without a
  pair it draws Meters by the fallback below.

The form's `NEW_SUBMIT` becomes `Enter opens it beside the focused one: meters, and a
goniometer pane for a pair`. `PAIR_HINT` becomes `L and R, loudness (line or digital); goniometer and
correlation in their own pane`, since a mic pair has no loudness.

#### The Meters view

**Bars under a loudness ballistic.** A true-peak bar per input the pane names, and a LUFS
bar per source those inputs resolve to (`sources()`, unchanged). Order: the **true-peak
block** first (every named input in pane order, sourceless ones included with their reason
row), then the **LUFS block** (sources in the order the pane first names one of their
inputs). A Pair pane: `Main L`, `Main R` | `Main`. An Inputs pane `1, 5, 2` with `1+2`
paired: `Input 1`, `Input 5`, `Input 2` | `Main`, `Input 5 mono` (generic names as `meter.rs` gives them). A pane `1, 5` with `1+2` declared and
input 2 captured: `Input 1`, `Input 5` | `Main`, `Input 5 mono` (input 2 is in Main's loudness but
has no bar, because the pane does not name it; the plate's TP max covers the source). A mic
pair or a mic: true-peak bars only, each with `true peak shown · no loudness on a mic
input`. This keeps §6.2's point: there is still **no per-input loudness bar**. The new bars
read true peak, which an input does have.

`bars_of()` builds both blocks, and the layout groups bars by variant. Each `Peak` bar keeps
its own flags row (overs, CLIP), so `SourceBar` loses `true_peak` and `sides`, and
`SideFlags` goes. A pane names its source's sides as its bars, except a pane that names
fewer inputs than its source has (`1, 5` with input 2 the pair of 1). Such a pane no
longer shows an unnamed side's overs and CLIP: its plate's TP max still covers the whole
source, and that side's own Inputs row and any pane naming it show the CLIP. The comment on
`LoudnessFacts` (`programme/loudness.rs`) that promises the unnamed sides' flags is
rewritten to say the plate's TP max is all the pane keeps of them. The LUFS bar's rows become reading, max, and the floor row (below). §6.2's
pair flags row (`2 overs L · CLIP R`) is gone, since each side's own bar says it.

**Under a peak ballistic nothing changes**: one bar per named input, one dBFS scale, as in
stage 4.

**Two scale columns.** Each block carries its scale column on its left. The true-peak block
has the 60 dB span under full scale (`SCALE_DBFS` numbers, `scale_w(Dbfs)`), and the LUFS
block has the EBU +9 span (`scale_w(Lufs)`, §6.2's labels and target tick). Each column has
its unit at its head, in the scale's colour and at its size: `dBTP` over the true-peak
column when every bar in the block reads dBFS-based true peak, and nothing when a
calibrated bar is among them (dBu, dB SPL: the numbers are still the shared span's, as in
§6.2, and each bar's own text carries its unit). The LUFS column is headed `LUFS`. Both
heads show at *full* and *no grid* and go with the numbers at *traces only*. The columns
keep their width at every step (§6.2), so only the hidden plate's room moves (below). §6.2's own four-number dBFS scale on a peak bar beside LUFS bars goes: the
block's column replaces it. `ScaleKind` stays as each block's kind
(`scale_w(kind, small)` is unchanged); only `ProgrammePane::scale_kind()` goes.

**Below the floor.** A LUFS reading under target −18 LU leaves the bar empty, so the bar
says so in two places. A small downward triangle (8 px wide, `text_dim`) is drawn inside
the bar's rect at its foot. The label `below −41` (the floor in LUFS,
`fixed(target − 18, 0)`, `small`, `text_dim`) goes in a text row of its own under the
bar's reading and max. That row is reserved on every LUFS bar whatever the reading, so
the pane's row count and every bar rect stay the same. Both are drawn while `reading` is
`Some(v)` with `v < floor`. A reading of — (no window yet) draws neither, since — already
says *nothing yet*. The
`max` ticks keep §6.2's rule (none outside the span). The span does not widen. 27 LU over
the bar's height is what gives a 1 LU resolution at the target, and the field case (a feed
34 LU under target) is what the mark is for.

**The plate fills its room.** In the Meters view the docked plate (`at: None`) is sized by
the pane and not by the stored scale:

1. The Meters view has no goniometer, so `layout()` and `dock()` in
   `programme/layout.rs` keep `has_pair` only for the chip strip under the bars (the strip
   is taken from the bars' height before the plate is fitted). `CHIP_W`, `SHORT_PANE_H`
   and the 1:1 split go; `CHIP_H` serves both views' strips. `layout()`'s
   doc comment is rewritten (5b). The bars' need is `Σ scale columns + bars × BAR_MIN_W`. The **fit scale** `s` is the
   largest that makes `loudness_plate::size(font, s, 0)` fit beside the bars in width and
   in the pane's height, capped at 3 (`SCALE_LIMITS`' top) and floored to 0.05 steps, so a
   pixel of resize does not flicker the text.
2. If `s ≥ 1`: *beside*, at the pane's right edge from the top, at `s`. Bars take the rest,
   widening as they do today (`col_w`, bar width clamp 6 … 40 px).
3. Else stage 4's ladder from its step 2: *above* at 1×, then slots dropped at 1×, then no
   plate.

A pair pane at 1700 × 900 gets a plate at 3× (about 876 × 376 px: `Int` about 82 px high).
A pair's Meters pane half of a 1920 px window wide (about 820 px of area; the bars need
about 380 px: 31.5 + 60 px of scale columns and 3 × 96) gets about 1.5× beside them. A
third of that window (under 640 px) docks the plate above at 1×.

The stored `scale` is now the **floating** size only. The grip on a docked plate floats it
where it is, at the scale the drag sets: one gesture, one **Ctrl+Z** step. To make that
happen, the loudness `PlateBox` of a docked plate is built with a new field `docked: bool`
(`PlateBox.scale`, the drawn scale, already exists). The box-aware mouse code
(`ac2-ui/src/view/panes/mouse.rs`, which already calls `b.position_for` /
`b.scale_clear_of`) handles either gesture (body drag or grip drag) on a docked box this
way. It first dispatches `PlateMsg::Move` to `position_for(rect.x, rect.y)`, which floats
the plate where it stands (only `Move` sets `at`, `plates.rs`). It then dispatches
`PlateMsg::Scale` with the box's drawn scale, and after that the gesture's own `Move` or
`Scale`. `PlateMsg::Move` carries no scale, so without the second message a 3× docked plate
would shrink to the stored 1× under the pointer, and without the first a grip drag would
change only the stored scale while the dock went on sizing the plate. All of these are drag messages of one gesture, so they make one undo step
(the drag grouping: `history.drag`, `start_drag` / `finish_drag` in `state/undo.rs`). A double-click docks it again (`PlateMsg::Reset`: `at: None`, scale 1, which a
docked plate ignores). The "stored scale is kept, so a larger pane brings it back" rule of
§6.2 goes, because a docked plate always takes what its pane gives.

**T on the Meters view.** The steps and toasts are §6.2's. One thing changes: at *traces
only* the hidden docked plate's room goes **to the bars** instead of staying reserved
(`ProgrammePane::layout` gains the chrome step and does not dock at *traces only*).
Stage 4 kept the room so that nothing moves, but an empty rectangle the width of the plate
is what made the field pane look broken, and the steps are an operator gesture, not a
value change, so the static rule (§6.2) is not broken. The toast
`Programme: no grid, target label, scale or loudness plate` says what went.

**The Goniometer view at T**: today's rule (`draw.rs`). The grid shows at *full* only, the
L/R labels at *full* and *no grid*, and the chip at every step (it is a reading). The toasts are §6.2's three: their wording
names the parts of the full step even on a pane that lacks one, as the SPL toasts do.

#### Ember decay on the form

The goniometer's fade is **Settings › Display › Ember decay** (app setting, `ui.toml`,
`prefs.ember_decay_s`, `step_ember_decay` in `ac2-ui/src/settings.rs`). The New / Edit
programme meter form gains the same row: label `Ember decay`, value text
`ember_decay_text()` (*0.4 s: a goniometer point fades out that long after it lands*),
a `Value::Choice` row like the form's other stepped rows
(the local `stepped(id, title, note, all, now, text)` closure in `meter_dialog.rs` with a
new `FieldId::EmberDecay` in `forms/mod.rs`). The closure takes `&[f64]`, `now: f64` and
`fn(f64) -> String`, and picks the index by exact `==`; the ember values are `f32`
(`prefs.ember_decay_s`, `EMBER_STEP_S`, `ac2_scene::gonio::EMBER_DECAY_RANGE_S`, `ember_decay_text(f32)`).
So this row does **not** go through `steps()`: widening first would miss its 1e-9 match
(`f64::from(0.4f32)` is 0.40000000596…) and list 0.4 s twice. A new
`settings::ember_decay_steps() -> Vec<f32>` builds the grid in `f32` (the steps of
`EMBER_STEP_S` across `EMBER_DECAY_RANGE_S`, the grid `step_ember_decay` rounds to). The
row widens each step, and `now` is picked out of that same widened list as the entry
nearest `prefs.ember_decay_s`, so it is the same `f64` value and `==` finds it (a
stored value between steps shows as its nearest step). The text function is wrapped.
`EMBER_STEP_S` stays private beside it. Test: 0.4 and 2.0 s each list once and select. The submit writes
`prefs.ember_decay_s`, and the Settings page keeps `step_ember_decay`. It is one value, so the form changes the same pref, and Display
shows the change. It sits under a third heading, after *How it reads*. `rehead`
(`meter_dialog.rs`) gives that one `format!("{} · Whose: {}", HOW_HEADING, THE_RIG)`
(`THE_RIG` = *the rig — all clients*, `settings.rs`). The new one follows the same
pattern: `DISPLAY_HEADING = "Display"` (the Settings page's name) with `THIS_APP`
(*this app*, `ac2-ui/src/settings.rs`). `DISPLAY_HEADING` goes to
`ac2-scene/src/programme/form.rs` beside `HOW_HEADING`, as `THE_RIG` stays in
`settings.rs` today, because §7 puts the value on the app and not on the rig. **Enter** applies it with the rest, **Esc** closes without
applying, as in every form. It shows for every Meter pick, because it is an app setting and
not a pane's, and it never sends `levels.set`.

**Range stays 0.1 … 2 s; the ring follows the decay.** The upper end is already more than
the app honours. `RING_CAPACITY` (`ac2-ui/src/state/programme.rs`) is `GONIO_POINTS × 32`,
which is 32 frames, or about 1.07 s at the daemon's `METER_FPS` 30. `EmberRing::retire`
waits `GONE_DECAYS × decay` (10 s at 2 s), but the ring has dropped each ember after about
1 s. At 2 s an ember disappears at 59 % brightness (e^−0.53). Above about 0.21 s
(1.07 s / 5) the ash stage is cut short, and above about 0.43 s (1.07 s / 2.5) it is
skipped entirely (2.5 … 5 decays, `STAGE_DECAYS` in
`gonio.rs`, which `ac2-plot` floors at 6 % alpha so ash stays visible until it is retired).
Stage 5 sizes the ring from the decay, so every ember lives its whole ramp:
`capacity = GONIO_POINTS × ceil(METER_FPS × GONE_DECAYS × decay)`, with `GONE_DECAYS`
unchanged at 5. At the 0.4 s default that is 60 frames (15 360 points). At 2 s it is 300
frames (76 800 points), which is one instanced draw of small additive points. The Pi kiosk
runs the default. `METER_FPS` moves from `ac2d/src/jobs/meters.rs` (`pub(crate)`) to
`ac2_proto::frame` beside `GONIO_POINTS` (5a), and the daemon uses that, so the app sizes
the ring from the rate the daemon sends at. `EmberRing` gains `set_capacity(n)`, which
drops the oldest points when it shrinks. A change of the setting calls it on every ring.
The range does **not** widen:

- Below 0.1 s an ember lives about 3 frames at 30 fps and flickers between frames.
- Above 2 s the cloud mixes more than 6 s of programme, longer than the 3 s short-term
  window and the slowest correlator window, so the shape no longer belongs to what the bars
  and the chip read now.
- The point budget grows linearly with the decay, and the Pi kiosk draws the same ring.

The operator asked for the control, not a longer tail. §5 now reads *a ring of the points of
the last five decays*.

#### Mic pairs

§2 now allows two mic inputs as a pair. Kinds must still match (mic + line is refused). A
mic pair reads like any input:

- **Bars**: per input on the input's own scale. That is dB SPL when its curve or
  calibration gives one, else dBFS, exactly as a lone mic's bar and Inputs row do today
  (`AppState::programme_scale` in `ac2-ui/src/state/programme.rs`). Under a peak ballistic: PPM / true peak. Under a loudness
  ballistic: true peak with `true peak shown · no loudness on a mic input` (`TRUE_PEAK_SHOWN · MIC_TEXT`,
  `sources()` already resolves a pair with a mic side to that reason). There is no LUFS
  bar, no LUFS scale column and no plate, because §6.1 meters loudness on line and digital
  sources only and the operator chose loudness for those.
- **Correlation and goniometer**: as for any pair, from the same daemon path. Correlation
  is gated per side in dBFS (the gate is about signal presence, not level), so calibration
  does not change it. What a mic pair's correlation means depends on the technique (a
  coincident XY pair reads high, a spaced AB pair low), and the user guide says so in one
  sentence. ac2 draws no conclusion from it (no verdict, as §4).
- The Inputs list pair row and the pane title are as for any pair. The New programme meter
  form's `FIRST_BAR_HINT` (*by name; mics show levels, no loudness*) still holds.

Mic pairs are declared as any pair (`check_pairs` in `ac2-proto/src/cal.rs` refuses only two
different kinds). Stage 5's rules read what `sources()` resolves.

#### The Inputs list rows

The operator, on the same screenshot: *"the input list looks bit disorienting… the top left
one dBTP and dBFS mixed and is not obvious what units the moving value is and clutter"*, and
the rule *"everything except bars themselves should be pretty static"*. Today a row
(`ac2-ui/src/view/chrome.rs` ~600–720) shows **two quantities**. The bar and its unlabelled
number are the always-on meter's 300 ms **RMS** (`MeterReading.rms_fill` / `text`, drawn by
`meter_sized` in `view/session.rs`), under the header `dBFS RMS · peak tick`. The lines
under it are the rig's **ballistic** reading and max (`ProgrammeInput::row_lines`), in dBTP
under a loudness ballistic. A pair puts `L`, a 56 px bar, its 64 px number, `R` and a
second bar on one line, about 260 px in a sidebar of about 210 px. The R bar is squeezed
and its number cut. The row's `cal` line, which is only the word `dBFS` for an uncalibrated
line or digital input (`ac2_scene::meter::DBFS`), lands under the correlation and reads as
its unit. Stage 2 promised that *"the input meters stop being an RMS bar with a peak tick
and become peak programme meters"* (*What the operator gets*), and the row never caught up.

Before (digital pair, `ShortTerm`):

```
Inputs  dBFS RMS · peak tick
Main · digital · pair (in 3, 4)
L ▮ −56.4 R ▮
L true peak  −50.6 dBTP
  max  −45.7 dBTP
R true peak  −48.6 dBTP
  max  −42.2 dBTP
correlation +0.31
dBFS
```

**Grammar.** One quantity per bar, and that quantity is the one the Programme pane's
per-input bar reads (§6.2, §6.3: `ProgrammeInput::reads(ballistic)`). The bar shows it,
the number beside the bar is the same reading with its unit, and the tick is its flying
peak (`ProgrammeInput::flying_peak_fill`, as on a pane's peak bar). The bar has the pane's
peak-bar colours: level green, the hot colour within `ac2_scene::meter::HOT_PEAK_DBFS`
(−6 dB, already `pub`) of full scale, and the fault colour while CLIP is up. The state is
`ac2_scene::meter::MeterState` (every state it has: `NoData` for the `—` row, `Silent`,
`Signal`, `Hot`, `Clip`), computed in the scene, so the app colours and never compares
levels. The RMS bar, its number and the `dBFS RMS · peak tick`
header leave the list. RMS stays where it is the quantity: the SPL and spectrum panes, and
the I/O page's device preview (`meter_sized` stays for that page). Both come from the same
`SessionLevels` frame, so nothing is lost.

Rows are monospace with tabular digits and fixed slots: number `NUMBER_SLOT` (6,
`−120.0`), CLIP (4) and overs `OVERS_SLOT` (10, `9999 overs`) as `row_lines` uses them, and
a new unit slot (6, `dB SPL`) that `row_block` pads to, since `row_lines` prints the unit
unpadded today. The bar takes what the row's width leaves after the fixed columns. Nothing
but the bar fill and the tick changes width or place as values move. Each side has its own
lines, prefixed by its mark:

```
<m> <bar ……………………>  <number> <unit> <CLIP>     the reading, one per side
<m> max         <number> <unit> <overs>    the ballistic's hold since R, one per side
<m> true peak   <number> dBTP              only when the bar does not read true peak
    correlation <value>                    pairs: the chip, warning colours on negative
    <note>                                 only when a row cannot read the ballistic
```

- **Mark column** `<m>`: `L` / `R` on a pair, blank on a mono row. It is one character wide
  on every row, so every number column lines up down the list. The mark moves into the
  scene's lines (today `chrome.rs` prefixes it).
- **max** and **true peak**: one line per side, so each side's overs and CLIP always have
  their slot, whatever the sidebar's width. These are the slots of today's lines, so a
  pair needs no more width than a mono row. `true peak` (this frame's) shows only under a
  ballistic whose reading is not already true peak (Type I / II, VU), so no number appears
  twice.
- **No reading** (no levels frame yet, a stale frame, or the session dialog previewing
  another device, the guard `session_inputs` applies today and `programme_of` gains): an
  empty bar, no tick, `—` in each number slot, and every slot kept, so the row has the same
  rect with and without data. Today the lines vanish until a frame arrives
  (`chrome.rs` ~675, the `ppm.is_some() || max.is_some()` gate); they stay instead.
- **Calibration**: a line calibration's text (`dBu · full scale +13.0 dBu · 3 h ago`)
  moves from its own line under the row (`chrome.rs` ~720) onto the label line, where a
  mic's curve already is. The scene's row label (`ac2_scene::meter`'s label for the row)
  appends it. An uncalibrated row has no unit line, because every
  number carries its unit. `InputRow::cal` is `None` for an uncalibrated line or digital
  input; `DBFS` stays for the I/O page.

**What the bar and number read, by ballistic** (every row on its input's scale: dBFS,
calibrated dBu or dB SPL). The header is the strong `Inputs` followed by the word of what
the bars read and `· max since R`. It is built from the rig's ballistic
(`self.daemon().map(|s| s.rig.levels.ballistic)`; deliberately the rig setting only, not the
frame's `meta.ballistic` that `programme_of` prefers, so it exists before any frame). It
goes through a new input-free `ac2_scene::programme::Ballistic::inputs_read(self)`: `TruePeak` under a loudness
ballistic, else `self`. Rows that cannot read VU say so on their own note. The header
changes only when the rig setting does. Right after a `levels.set` the header can name the
new ballistic for one meter frame (≤ 33 ms) while the rows still read the old one. Each
row's numbers still carry their own unit, so no number is misread, and taking the header
from the frame instead would leave it empty until the first frame:

| rig ballistic | bar and number | unit (uncalibrated) | header after `Inputs` |
|---|---|---|---|
| Type I, Type II | that PPM | dBFS | `Type I · max since R` |
| true peak | true peak | dBTP | `true peak · max since R` |
| VU | VU; a row without a line calibration reads true peak with its note (`true peak shown · VU needs a line calibration`) | dBFS / dBTP | `VU · max since R` |
| Mom / Short loudness | true peak (loudness is a source's, in the Programme pane, §6.2) | dBTP | `true peak · max since R` |

The words are the Programme pane title's (`ac2_scene::programme::Ballistic::word()`): under a loudness ballistic
the inputs read true peak, so the header says `true peak`, not `Short loudness`. The hover
tip keeps the keys sentence, and under a loudness ballistic adds *loudness is metered per
programme in the Programme pane*. That is too long for the header line, which must not
wrap and push the rows down. `R` is the reset key's label (`first_chord`).

After (same pair, `ShortTerm`, one over on L), then a mono line input with a line
calibration under Type I, then an uncalibrated mic pair under true peak:

```
Inputs  true peak · max since R
Main · digital · pair (in 3, 4)
L ▮▮▮▮▮▮▮──────────  −50.6 dBTP
R ▮▮▮▮▮▮▮▮─────────  −48.6 dBTP
L max     −45.7 dBTP     1 over
R max     −42.2 dBTP
  correlation +0.31

Inputs  Type I · max since R
Vox · line (in 5) · dBu · full scale +13.0 dBu · 3 h ago
  ▮▮▮▮▮▮▮▮▮▮───────   +2.1 dBu
  max      +6.0 dBu
  true peak −6.4 dBTP

Inputs  true peak · max since R
Overheads · mic · pair (in 7, 8)
L ▮▮▮▮▮▮───────────  −31.2 dBTP
R ▮▮▮▮▮────────────  −33.0 dBTP
L max     −18.4 dBTP
R max     −19.9 dBTP
  correlation +0.62
```

(Before, the mock is a sketch: today's reading line also has its 4-character CLIP slot,
and `chrome.rs` adds the `L` / `R` prefixes.)

A mic pair follows the same grammar. A calibrated mic reads dB SPL in every slot (`94.2 dB
SPL`, `max 101.0 dB SPL`), and the true-peak line stays in dBTP, because true peak is a
digital quantity on every scale (`true_peak_text`). Under a loudness ballistic a mic row
has no note. Its true-peak reading is what every input reads there, and the reason it has
no loudness belongs to the Programme pane's bar.

**Same words everywhere.** `max`, `true peak`, `correlation`, the units and the flag forms
are the Programme pane's and the CLI's (`ac2 levels`). `row_lines()` becomes
`programme::row_block(inputs: &[ProgrammeInput], ballistic)`, which builds the lines above for a mono row
or a pair from the same `ppm_text` / `max_text` / `true_peak_text` helpers the pane uses, so
the two surfaces cannot drift. It returns one `RowBlock`: per side the bar fill, the
flying-peak tick and the `MeterState`, the text lines in their fixed slots, and the
correlation chip's text and warning state. The correlation stays the chip
from `correlation_line`, with its warning colours on a negative value.

#### Wire and persistence

- **Proto (5a):** `LayoutPane` gains `programme_view: ProgrammeView` (the wire enum above, `meters` |
  `goniometer`, `rename_all = "snake_case"`), required like its four siblings (the struct is
  `deny_unknown_fields`, and every pane carries every kind's view). `PROTO_VERSION` 70 → 71,
  `WIRE_LOCK`, `tools/protocol/fixture_messages.py`'s pane gains the key, and
  `samples/builders.rs` gets it too.
- **Old layouts:** `ac2_traces::old_layout::programme_views(v)`, beside
  `plates_add_loudness`. Like `plates_add` it recurses through the value, but it matches
  the `{"type": "pane"}` wrapper object (not the pane inside it), because a pair pane is
  replaced by a split there. It adds `"programme_view": "meters"` to every pane that lacks it. A
  `"kind": "programme"` pane whose `content` is a `pair` is instead rewritten one level
  up, at its tree node: the walk matches the wrapper `{"type": "pane", "pane": {…}}` whose
  inner pane is such a pane, and replaces that wrapper in place (the tree root, or an `a` /
  `b` slot) by `{"type": "split", "axis": "row", "ratio": 0.5, a: <pane, goniometer>, b: <pane,
  meters>}`. Both copies keep the pane's content, chrome and levels. The goniometer copy
  drops its `plates` key (it defaults whole, as `old_layout` already relies on), because a
  goniometer has no plate. A tree may hold at most `PaneLayout::MAX_PANES` (16) panes;
  beyond that `check` fails, and salvage would drop the whole layout. So the walk counts
  the panes first and splits pair panes only while the running total stays at or under 16.
  Any further pair pane stays one pane with `meters`. Test: 15 panes, 2 of them pair
  panes. This turns the stage-4
  pane into the two panes a new pair brings. The module header gains *session format 34
  and rig settings version 6*. Callers run it after `plates_add_loudness`:
  - **sessions:** `session::VERSION` 34 → 35. An older autosave is set aside as
    `autosave.v34` as at every bump, and `ac2 session salvage` (`salvage/read.rs`) calls
    it wherever it calls `plates_add_loudness` today: per measurement layout (salvage does
    not read the session's own `layout`);
  - **the rig file:** `VERSION` 6 → 7, a new `rig/v6.rs::upgrade` calling it on each
    `meters[].layout` (as `v4.rs` calls `plates_add_loudness`), appended after
    `v5::upgrade` in every branch of `rig/file.rs`;
  - **`ui.toml`:** `PaneEntryFile` gains `#[serde(default)] programme_view`, which defaults
    to Meters. This follows the file's own convention, one serde default per key (`spl_view` names a default fn; `ProgrammeViewFile` derives
    `Default` = Meters, so a plain `#[serde(default)]` does). Prefs carry no version, so an old shape cannot be detected without a
    reader branch that would live forever (a shim). A stage-4 pair pane kept in `ui.toml`
    therefore reads back as **one** Meters pane, and **G** or **P** on its pair row gives
    the goniometer. The two-pane rewrite is for the versioned stores only. A test pins
    that a stage-4 `ui.toml` pair pane reads back as one Meters pane. This is accepted,
    not hidden: the 5d user guide's Programme section says that an app layout from before
    the views opens a pair pane in Meters view and that **G** or **P** brings the
    goniometer. No one-time toast, since detecting the old shape is the reader branch
    ruled out above.
- `PaneLayout::check` is unchanged. A `goniometer` view on a non-pair pane is valid and
  draws Meters (above).

#### Keys and words

- **G / Shift+G** (`CommandId::NextView` / `PrevView`) gain `Scope::Programme`: the
  `NextView` scope list in `keys/mod.rs`, two rows in `keys/table.rs`
  (`(C::NextView, S::Programme, k(K::G))`, `(C::PrevView, S::Programme, sh(K::G))`),
  `hint(C::NextView, "view", 70)` in the Programme hints (it ranks between reset all and
  grid), and `keys/keywords.rs` adds `meters` (`goniometer` is already listed). `G` is free in that scope
  (only R, Shift+R are bound there). `keys/tests.rs`'s `g_steps_the_views_of_every_pane`
  asserts G is unbound in `Scope::Programme` today; 5c moves Programme to the bound arm and
  rewrites its comment.
- **User guide (5d):** the key table's G row names Programme (*meters ↔ goniometer*). The
  *Programme pane* section is rewritten for the two views, two panes per pair, the
  true-peak + LUFS blocks, the floor mark and the plate that fills its room. Also: the
  *Moving the plates* sentence on the docked loudness plate (pane-sized, grip floats it),
  *Grid, labels and cursor (T)* (the plate's room goes to the bars at the third step, and
  the Goniometer view's steps), *Input meters* (the row grammar, the header by ballistic and the before / after mock of §6.3 *The Inputs list rows*), the mic-pair sentence (§2), and *Display › Ember decay* (also on the programme
  meter form).
- No new key, so no new hint beyond G's. Esc only closes. No Ctrl chord.

#### Sub-stages

Each is one agent, in order. 5b and 5c each start from the scene text.

- **5a proto + formats** (`ac2-proto`, `ac2-traces` `old_layout.rs` / `session.rs`,
  `ac2d/src/rig/{file.rs,v6.rs}`, `ac2-cli/src/cmd/salvage/read.rs`, `tools/protocol`):
  `ProgrammeView`, `LayoutPane.programme_view`, PROTO 71, `WIRE_LOCK`, session format 35,
  rig file v7, `programme_views` and its callers. Not a wire fact but landed here because
  the app needs it: `METER_FPS`, the daemon's meter frame rate, moved to `ac2_proto::frame`
  (`ac2d/src/jobs/meters.rs` uses it, and the rustdoc link in `ac2d/src/fanout.rs` follows).
  *Tests:* serde names; `WIRE_LOCK`; `programme_views` on a tree with a pair Programme
  pane (becomes a row split, goniometer left, plates reset on that copy), an Inputs
  Programme pane (`meters`), a Transfer pane (key added), and a tree already carrying the
  key (unchanged); salvage of a format-34 session with a pair pane inside a measurement's
  `measurements[].layout` (the only layout salvage reads); the session round trip of a
  format-34 autosave set aside, as at every bump; rig v6 → v7 upgrade of
  a meter layout; the session round trip with a `goniometer` pane.
- **5b scene** (new tests go to `programme/tests/blocks.rs`, since `tests/bars.rs` is at
  646 lines; `ac2-scene`: `view.rs`, `programme.rs`, `programme/{layout,bars,draw,
  loudness_plate,form}.rs`, `gonio.rs`). `draw.rs` is at 676 lines, so the views move to
  `programme/meters.rs` and `programme/gonio_view.rs`, which keeps every file under 1000.
  Changes: `ProgrammeMode` with `next/prev(has_pair)`, `ProgrammePane.view`, `title()`
  per view, `bars_of()` blocks, `SourceBar` slimmed, block scale columns and their heads,
  the floor mark, the fit-scale dock, the hidden plate's room to the bars, the Goniometer
  view's square and chip, `PlateBox.docked`, the form's words and `DISPLAY_HEADING`,
  `ring_capacity(decay_s)` from `ac2_proto::frame::METER_FPS` and `GONE_DECAYS`, and
  `EmberRing::set_capacity`.
  *Tests (scene text first, headless):* bars and their order for a Pair pane, `1, 5, 2`,
  `1, 5` with input 2 unnamed, a mic pair, a mic, under `ShortTerm` / `Momentary` and under
  `TruePeak` (unchanged); the column heads (`dBTP`, none with a calibrated bar, `LUFS`);
  `below −41` and the triangle present at −56.5 with target −23, absent at −40.9, at —, and
  at target −22 reading `below −40`; every bar rect identical across those readings; the fit
  scale at 1700 × 900 (3×), at a half-window pane (between 1 and 3), at a third-window pane
  (above at 1×), floored to 0.05; a docked box has `docked`, a floating one not; at *traces
  only* no plate box and the bars' right edge at the pane's; the Goniometer view: the square
  is the largest centred in the pane less the chip, the chip as wide as the square or its text, whichever is wider, chip
  alone under `GONIO_MIN_W`; Goniometer stored on a non-pair pane draws Meters; titles of
  both views (the Meters title of a pair pane has no correlator window), and the Meters
  title of a pane without a pair; `programme_name` toasts
  unchanged; `ring_capacity(0.4)` = 60 frames and `(2.0)` = 300; `set_capacity` keeps the
  newest points; an ember at 2 s decay is still in the ring at 9 s and retired after 10 s.
- **5c ui + plot** (`ac2-ui`: `state/panes/views.rs`, `queries/select.rs`,
  `state/programme.rs`, `scenes/programme.rs`, `meter_dialog.rs`, `state/meter_form.rs`,
  `state/panes/plates.rs`, `state/panes/meas_layout.rs`, `view/panes/mouse.rs`,
  `state/undo.rs`, `forms/mod.rs`, `prefs/layout.rs`, `prefs/file.rs` (916 lines; the
  enum adds about 10, under the cap; its conversions go to `prefs/layout.rs`'s
  `PaneEntryFile::of` / `parse`),
  `state/queries/inputs.rs` (where
  `InputRow::cal` is built), `view/chrome.rs`, `keys/*`, the regenerated per-scope key
  table in `docs/user-guide.md` (`keymap_doc.rs` checks it against the binding table, so it
  changes with the two new rows) and the `Pane | G steps` row; `ac2-plot` golden images).
  `view/chrome.rs` is at 915 lines, so the Inputs list moves to its own
  `view/inputs_list.rs` first. Changes: G on Programme, two panes per pair, the form's Ember decay row, the ring
  re-sized on a decay change, docked-grip floating, `ui.toml` key and its read rule, and the
  Inputs list rows (5b adds `row_block` and the header text to `ac2-scene`, with
  scene-text tests for every ballistic row of the table, mono, pair and mic pair, and the
  slots' widths constant over readings −∞, −120.0, −0.1, +3.2, with overs 0 / 999+).
  *Tests:* G steps Meters ↔ Goniometer on a pair pane and toasts on an Inputs pane (scene
  text), and one Ctrl+Z steps back; P on a pair row opens two panes (goniometer left,
  meters focused) and one pane with no room for two; a pane-list Meter row turns one pane;
  one Ctrl+Z removes both panes, and neither keeps a `meas`; a wire layout with a
  `goniometer` pane round-trips `ui_view` → `wire_pane`, and a `ui.toml` one round-trips
  `PaneEntryFile`; the form's Ember decay row changes `prefs.ember_decay_s`, Display shows it, and Esc leaves
  it; a grip drag on the docked plate stores `at` and `scale` in one undo step, and a
  double-click docks it; a stage-4 `ui.toml` with a pair pane reads as one Meters pane; the two
  assertions `cal == Some(DBFS)` in `state/tests/session/meters.rs` flip to `None`; the Inputs
  list in an egui snapshot: a pair row is two bar lines whose numbers sit in the mono
  row's column, no RMS bar or number, no bare `dBFS` line, the header follows a ballistic
  change, and a row's rect is unchanged between a quiet and a clipping frame; an e2e from an empty daemon (declare a line pair, `ShortTerm`, P, read both
  panes' scene text); a reducer case declaring two mics as a pair on `ShortTerm`: the Inputs row has two dBTP bar lines and the correlation, P opens two panes, the Meters pane has true-peak bars only (no LUFS column, no plate), named `slow_…` since it waits on real Leq windows. Golden images: the Meters view of a pair on `ShortTerm` at
  1700 × 900 (plate at 3×) and at 600 × 500 (plate above); a reading below the floor; a
  line + mic Inputs pane (both blocks); the Goniometer view at 900 × 900 and 1600 × 600
  (square centred) on uncorrelated noise; embers at decay 0.4 and 2.0 s, where the 2 s
  image still shows the tail at 1.5 s, which today's ring drops. These replace
  `programme_traces_only.png`, `programme_plate_beside_*` and `gonio_embers_decay_*` in
  `ac2-plot/tests/reference`.
- **5d docs**: the user guide prose above (5c has already regenerated the key tables), `docs/protocol.md` (the layout pane's key), and
  this section's *as built* note.

**Not in this stage:** a combined view, a plate per
source, a relative LU readout, an Int bar.

**Settled with the operator (2026-10-26):** the field pane was at **T**'s third step (the
screenshot has no scale numbers and no goniometer grid), which confirms *Why the field pane
had no plate*. A pair's Meters view keeps the correlation chip under its bars.

**As built (stage 5).** Where the code took a different path than the text above:

- `docs/protocol.md`'s layout key landed with 5a, beside the wire change it documents.
- The Inputs list left `chrome.rs` for `ac2-ui/src/view/inputs_list.rs`, and it landed with
  5b, as did the `ac2-plot` golden images: the scene change redraws both, so each image and
  snapshot was blessed once. The list takes one row width for every bar line, so bars and
  numbers line up down the list whatever egui's layout pass gives a row.
- `row_block` takes the pair's correlation as a third argument and builds the chip there.
- A short true-peak column thins its labels to every 30 dB (`peak_ticks`), since at the
  dropped-rows size the 10 dB labels overprinted. The dropped-rows image keeps a one-slot
  drop.
- The ember rings are resized in the frame tick (`retire_embers`) when their capacity
  differs from the decay's, which covers every way the decay changes (Settings, the form, a
  reloaded `ui.toml`) without a call per setter. The form's row picks its index as the
  nearest step of the `f32` grid directly.
- The mic-pair case is an e2e against the fake daemon (`tests/it/ui/programme.rs`,
  `a_mic_pair_on_short_loudness_reads_true_peak`) and not named `slow_…`: the fake's
  meters answer at once, so it waits on no real Leq window and runs in 0.4 s. The line-pair
  e2e is `programme_pane_of_a_pair`, which reads both panes' scene text.
- The header-by-ballistic rule is pinned by the scene's `inputs_header` test rather than an
  egui snapshot per ballistic; the snapshots show the true-peak header.

## 7. Where the settings live

- **Rig settings** (daemon, `levels.set`, beside the system max level; `ac2 levels`):
  ballistic, correlator window (s), over threshold (dBTP), correlation gate (dBFS), and
  from stage 4 the loudness target (LUFS, §6.2: it colours the loudness bars). The
  daemon computes these quantities, so it owns their parameters and every client reads
  alike. `levels.set` is a rig edit under decision R3: it names the rig rev it was made on
  and is refused with `conflict` when another client changed it first.
- **App Settings**: ember decay only — how a shape fades is how this operator likes to
  look, and no number depends on it. From stage 5 the programme meter form shows the same
  row (§6.3), the same value.

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
5. **Meter and goniometer views** (§6.3, built): from the 2026-10-26 field report.
   Programme panes get Meters / Goniometer views stepped by **G**, and a pair brings one
   pane of each. Under a loudness ballistic the Meters view shows a true-peak block plus a
   LUFS block, each with its own scale column, a mark below the floor, and a plate sized
   from its room. Ember decay is also on the programme meter form, and the ember ring
   follows the decay. Inputs list rows: one quantity per bar (the ballistic's reading, not RMS), with its unit beside it, fixed slots and a header by ballistic. How a mic pair reads. Layers:
   *5a proto + formats* (PROTO 71, session format 35, rig file v7, `old_layout::programme_views`,
   `frame::METER_FPS`),
   *5b scene*, *5c ui + plot*, *5d docs*. The tests are listed in §6.3 *Sub-stages*. No
   `ac2-core` and no daemon math change.

Each stage is one landed batch.
