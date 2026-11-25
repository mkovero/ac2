# Tracked clocks: a transfer function with a rig whose interface runs free

Status: **designed, not built** (2026-11-25). Extends `remote-inputs.md` (§3.5 there is the
background and the measurements; this note puts the resampler on the taking rig instead of
the source). Built beforehand: `ac2-resampler` (`Resampler`, `Kernel::measurement`, `Servo`).

## What the operator gets

The CM4 with its USB measurement mic (+7.3 ppm, no clock input) stands at FOH. pupu plays pink
noise into the PA and has its loopback. Today pupu can take the CM4's mic for spectrum, RTA
and SPL, but a transfer function between pupu's loopback and that mic stops: *CLOCKS NOT
LOCKED +7.3 ppm*, and nothing the operator can do on site changes that. With this stage:

1. The operator adds the CM4's mic to pupu's session and makes the transfer function, as today.
   Nothing else: no setting, no calibration step, nothing played beyond the measurement's own
   pink noise (operator, 2026-11-25).
2. About 25 s after the stream starts the CM4's heading in the measurement list reads
   *TRACKED +7.3 ppm* and the transfer function averages. pupu resamples the mic's stream onto
   its own sample clock, steered by the capture stamps both rigs already send; the CM4 is
   unchanged.
3. While the transfer function runs with signal, its measured delay trims the resampler; once
   the audio has matched the clock model for half a minute, its pane reads *LOCKED +7.3 ppm*.
   With the noise off it stays LOCKED for about two minutes, then reads TRACKED again.
4. A pair whose clocks agree (both interfaces on the Orion) reads *clocks agree* as today and
   its samples stay bit-exact: tracking starts only where the stamps say the clocks differ.

The delay still includes the unmeasured offset between the rigs (`remote-inputs.md` §4); the
phase above a few kHz under TRACKED wanders over minutes (§4.4), which the Connection page
quantifies.

## 1. Where it runs: the taking rig's joined feed

Rig A takes rig B's input. A resamples B's stream in its joiner (`ac2d::remote::join`, on the
fan-out thread), not B at its source (operator, 2026-11-25). Everything the servo needs is on
A already: B's capture stamps in every frame header (`AudioHeader.capture_ns`), A's own stamps
and stamp line on the fan-out thread, and the transfer function whose delay trims the servo
(§5). So trim needs no channel back to B, and B serves every taker the same raw stream. A pair
whose clocks agree pays the link offset's extra 0.5 ms (§3.3) and nothing else. The servo
re-times B onto **A's sample clock**, whichever of the two runs free; spectrum, RTA and SPL on
B's input are unaffected at a few ppm.

## 2. The servo's time axis on A

`Servo` takes stamps as (device input index, `MediaTime`) and hands out a position in device
input samples for output index k at media time k / rate. On A:

- **Device input index** is B's sample index (the header's `start_sample`).
- **Output index** is A's sample index (the joined feed's); **media time** is A's sample index
  / rate. The servo is unit-agnostic: "media time" here is A's sample time, not system time,
  and `Servo::status(now)` is asked at A's newest sample / rate. `MediaTime` is u64 ns: from
  A's integer index the joiner forms it in u128 (`index · 10⁹ / rate` overflows u64 after
  53 h at 96 kHz); from a placed capture (f64 samples, exact to 2⁵³) it rounds to ns.
- **A placed capture** is B's block capture on A's sample timeline: B's capture time on system
  time, onto A's stamp clock through A's stamp line (`CaptureClock::stamp_line`, a fit of A's
  stamp clock against its system clock over the last minute, refitted every 250 ms), onto A's
  sample index through A's newest stamped block at the nominal rate. That is
  `Joiner::local_of`, as the anchor and the cross-rig rate windows use it today.
- **One stamp a second**, as `stamp_white_s` is defined (the sd of a one-second mean;
  `observe_stamp` weights by the seconds since the last stamp). Second j is `[j·rate,
  (j+1)·rate)` of A's index; a B frame (one channel per rig) belongs to the second its placed
  capture falls in; a second closes when a capture passes its end, and its stamp is the mean of
  its captures, each moved to its newest B index at the nominal rate. Frames sharing an A block
  then count as often as they occur, whatever the periods or bunching. **Only full seconds are
  fed** (frames covering ≥ 90 % of a second of B): a one-frame second carries one block's noise
  (≈ 135 µs) weighted as 7 µs. A second cut by a servo reset or by `capture_system_ns` going
  `None` is dropped. The nominal-rate move leaves a constant rate × mean age, 3.6 µs at
  +7.3 ppm: part of the unmeasured offset (§9).

**What a stamp errs by**: B's stamp path, A's, and the wander between the hosts' system
clocks. On the rigs (`timing-study.md` §7b, servo docs) stamps alone leave a locked interface
on a software-PTP host at 15.9 µs sd, the USB mic on a hardware-PTP host at 7.9 µs sd (a lower
bound), and the hosts' system clocks 9 µs sd apart over an hour: RSS ≈ 20 µs, **an estimate of
20–25 µs sd** over minutes (the rig acceptance measures it, §10). The servo removes the fast
parts; nothing in the stamps removes the slow ones. *Rejected:* a second servo smoothing A's
own stamps (two linear stages remove the same bands as one); a nominal-slope line for A
(assumes a lock no evidence shows, `remote-inputs.md` §3.1).

**The servo's configuration** (design values, checked on the rig in stage 1): `wander_sd_s` =
√(15.5² + 27.8² + 9²) ≈ **33 µs**, `stamp_white_s` = √2 × 5 ≈ **7 µs** (each host's one-second
white part), `acquire_min_s` 0.9 s (two stamps a second apart span 1 s ± one B period: at the
default 1.0 half the time a third would be waited for); the rest default. `Servo::new` checks
them. Acquisition: rate sd under 1 ppm (σ√(2(1 − e^(−T/100 s)))/T at σ = 33 µs, with the white
part) after about **21 s**, not 11.3 s (to be measured in stage 1).

**B's capture on system time in the header.** B publishes its stamp map only every 5 s
(`CLOCK_EVERY_S`), so mapping through it steps periodically, which the servo would read as
noise. The header gains **`capture_system_ns: Option<u64>`**, B's capture through B's live
stamp line (as `LevelsMark.system_ns` already is); `None` without capture stamps or line.
`capture_ns` stays (the anchor's `ArrivalReason` tells *no stamps* from *no map yet* by it);
A uses the new field for the anchor, the rate windows and the servo. About 27 bytes a frame
(a named msgpack key and a u64; §8).

## 3. The joiner, tracked

### 3.1 Structure and ownership

The **control thread** decides when tracking *starts* (§4.1) and what relation is *shown*
(§4.3); the **joiner** owns the servo, stamps, engagement, re-acquire, resamplers, trim and
feeder. They talk through one new fan-out message and the existing report:

- `FanoutMsg::Track { rig: RigId, stream: u32 }`: start tracking that rig's stream. The
  fan-out thread calls `Joiner::track`; a `stream` other than the rig's current one is stale,
  and a second `Track` for a started stream is a duplicate: both are ignored.
- **A rig's stream** is bumped exactly where the joiner restarts the rig's rate windows today
  (`restarted` in `Joiner::audio`): any input of the rig (re)anchors — a new incarnation, a
  re-anchor, or a link-offset change. A bump restarts (re-anchors) **every** input of the rig,
  so none keeps a stale `k`; the rig's tracking (servo, resamplers, engagement) is dropped
  with the stream.
- `RigReport` (in the once-a-second `RemoteMsg::Joined`) gains `tracking: TrackReport`:

```rust
pub(crate) struct TrackReport {
    /// This rig's stream in this joiner, counted from 0; bumped on every restart.
    pub(crate) stream: u32,
    /// `None` until `Track` started the servo for this stream.
    pub(crate) servo: Option<ServoReport>,
}

pub(crate) struct ServoReport {
    /// At A's newest sample.
    pub(crate) status: ac2_resampler::Status,
    /// The joined feed takes this rig's inputs through the resamplers.
    pub(crate) engaged: bool,
    /// The servo's last reset: `Full` (`reset`: the start, a re-acquire) or `Phase`
    /// (`reset_phase`: a GAP, a break on A, an unsynced spell's end), and the s since it.
    /// A `reset_phase` on a servo not acquired since its last reset leaves it unchanged.
    pub(crate) last_reset: (ResetKind, f64),
    /// Since the last stamp fed, s; the joiner's own clock, kept across resets
    /// (`Status::since_stamp_s` is `None` after any).
    pub(crate) since_stamp_s: Option<f64>,
    /// Re-acquires since the start.
    pub(crate) reacquired: u32,
    /// How far back from A's newest sample the placement is not vouched for, s (§4.4).
    pub(crate) unvouched_s: f64,
    /// Which rig's stamps stopped, when none are fed (§4.3).
    pub(crate) stamps_from: Option<RigId>,
}

pub(crate) enum ResetKind { Full, Phase }
```

Per rig (`RigState`): one `Servo`, built at `Joiner::new` and idle until `track`. Per remote
input (`InputState`): one `Resampler`, all of a joiner sharing one
`Arc<Kernel::measurement()>` (0.9 MB table), B's samples pushed from `track` on (index =
B index − `base`, `base` being B's index at the resampler's last reset), so the history is full
long before engagement. **Everything allocates at setup**: `Joiner::new` runs on the control
thread (`control/remote.rs`) and builds the kernel, the resamplers, a pull buffer of one A
period per input and the servos; the kernel's last `Arc` clone stays with the session there.
**Retention** (`Resampler::new`'s `max_backlog`), sized before the link offset is known:
`MAX_LINK_OFFSET_S` (0.5 s) + `MAX_HELD_S` (1 s) + one burst after a credit stall (≈ 0.7 s),
within a 2¹⁸-sample ring at 96 kHz (2 MiB per input), 2¹⁷ at 48 kHz: `max_backlog` =
ring − `max_block` (A's period) − taps, since `Resampler::new` rounds the sum up to a power of
two. Cost while tracked: 56 multiply-adds per sample.

`Status` gains **`rate_sd_ppm: Option<f64>`** (√P[RATE][RATE], in ppm) in stage 1: engagement
needs it (§4.1).

### 3.2 The tracked path

Before engagement an input runs exactly as today: the ring at `local = remote + k`, `late()`,
`slip()`, `release()`, bit-exact. On engagement the input switches to the tracked path at the
next released block, the **hand-over block**: its whole column is a gap (one mark, averages
restart once). `ColumnMark` gains **`tracked: Option<i64>`** (resampled; the servo's last
acquired rate in ppb, an integer so the marks keep `Eq`), carried from the hand-over block on
**every** block of a tracked column with the engaging rate first (engagement implies acquired),
then held while the servo re-acquires; the recorder turns it into `RemoteTiming` (§8).
Resampled output starts with the next block.
The k-placed feed has slid since its anchor by the rate times that time: 0.18 ms at 25 s for
+7.3 ppm, 0.2 ms at 82 s for +2.5 ppm. The transfer function was stopped by the relation
until then, so its delay returns by `remote-inputs.md` §3.4's relock
(`tracking_relocks_after_a_far_jump` covers a move of this size).

On the tracked path, for **every** released A block `[n, n + len)` the joiner asks
`servo.block(n, len)` → `(Pos, Step)`, so the handed positions continue whatever the audio
did; then pulls `available(pos − base, step)` outputs (at most `len`) into the column and marks
the rest missing (zeros, as today's gap). `pos − base` is a signed difference formed before a
`Pos` is made (`Pos` is unsigned). While `block` returns `NotReady::Acquiring` the whole block
is a gap.

- **k** is not used: the k-branch of `Joiner::audio` (late, slip, `arrival_margin`) is skipped
  for a tracked input; the system-time anchor's check still decides whether it may be tracked.
  On the tracked path, **late** = a released block with `available < len` outside a reset's
  head (cue *link offset too small*, counted as today); **`arrival_margin`** = (B's newest
  pushed index − (`pos` + lookahead)) / rate at the block's end. **No slips**: a tracked input
  never sends a slip mark (`slipped` stays for pairs not tracked).
- **`BeforeHistory`** (`available` with `pos − base` < 27, the kernel's history) after every
  resampler reset is the reset's head: those outputs are a gap until `pos − base` ≥ 27. Only a
  position before retained input that had been there (oldest > 0) is a fault: a gap and one
  warning per stream; it cannot occur while the servo tracks (retention is 1 s past the link
  offset, the servo errs by µs).
- **Link-side loss** (a hole in B's `start_sample` without `AudioFlags::GAP`): B's index still
  counts B's samples, so the servo keeps its phase; only that input's resampler resets
  (`base` = the new start), a gap until the kernel's 27 + 28 samples are there.
- **Loss on B** (`AudioFlags::GAP`): `Servo::reset_phase` (rate kept, phase re-acquired from
  two full seconds of stamps: 2–3 s of gap, the cut second dropped); every input of that rig
  resets its resampler.
- **A break on A** (a local block whose flags break continuity): A's index may have stepped,
  which steps every placed capture. Every servo `reset_phase`s; the resamplers keep their input.
- **A new incarnation** (B's session restarted) or a re-anchor: a new stream, untracked;
  tracking starts again by §4.1. Cost: another ~21 s of acquisition after a source restart.
- **A system clock unsynced** (`system_synced` false, or no `capture_system_ns`): no stamps
  fed (an unsynced clock may step), blocks handed out on the held rate (`Mode::Coasting` after
  3 s); on return the servo `reset_phase`s.

### 3.3 The link offset

A tracked output at position p needs B's samples up to p + 28 (`Kernel::lookahead`): 0.29 ms
at 96 kHz, 0.58 ms at 48 kHz. The servo places B by its smoothed phase, which lies off B's true
capture time by its error (20–25 µs sd estimated, §2), and audio arrives by its true capture
time. So the default link offset (`default_link_offset`, 2 × B's period + A's period + 2 ms)
gains **lookahead + 0.2 ms** (design value) whenever the session has a remote input: 10.5 ms
at 256 frames / 96 kHz (from 10 ms), 66.8 ms at 1024 / 48 kHz (from 66 ms). Unconditionally,
because a link offset that changed at engagement would restart every remote input of the
session (`Joiner::source`). `remote-inputs.md` §4's defaults are updated with it.

## 4. The relation: start, engage, re-acquire

`PeerRelation` gains **`Tracked`** (stage 1) and **`Locked`** (stage 2), both units (§8).

### 4.1 Start and engage

**Start** (control thread, in `remote_publish`, before it publishes): held reads **Not
locked** for B by `CrossRate` or `OwnClock` (not `OtherRate`: the resampler covers ±0.1 %),
every input of B is on `Anchor::SystemTime` (so no *Can't tell* is ever tracked), and no
`Track` was sent for B's current stream: send `FanoutMsg::Track { rig, stream }` and remember
it in `RemoteControl.track_sent: BTreeMap<RigId, u32>`, cleared with the frozen stop reaches
(§4.4) wherever `self.remote.joined = None` (a new joiner counts streams from 0). A sent
`Track` counts as a servo acquiring from that same publish (§4.3), so Not locked never shows
between sending and the next report.

**Order in one push** (joiner): close a second and feed its stamp (§2), the re-acquire check
(§4.2), the engage check; engagement takes effect at the next released block; the report reads
the state after all three.

**Engage** (joiner, at each stamp), sticky for the stream (an interface that locks again
later stays TRACKED near 0 ppm): the servo is acquired and

  max(0.5 ppm, 3 · `rate_sd_ppm`) < |`rate_ppm`| < 1000 ppm.

`rate_sd_ppm` is 1 ppm at acquisition (`acquire_rate_ppm`), then falls about as
σ√(2(1 − e^(−T/100 s)))/T (σ = 33 µs): 0.83 ppm at 28 s of stamps, 0.37 at 100 s, 0.17 (3 sd
= 0.5 ppm) only at 280 s. Times (to be measured in stage 1):
- **USB mic**, B's own line already `Measured` (B up over a minute): `OwnClock` at the first
  report (≤ 1 s), first full second ≤ 2 s later, acquisition ≈ 21 s, ticks ≤ 2 s: **24–26 s**.
  A B just started has no own line yet and waits for the windows like the next rows.
- **Own lines within ±2 ppm, cross rate 3.5 ppm and more**: the 40 s window (36 s,
  `READY_SHARE` 0.9) then acquisition: **≈ 58 s**.
- **Cross rate 2.5 ppm**: the 60 s window (54 s), then 28 s of stamps: **≈ 82 s**; nearer
  2 ppm, longer by the curve.
- **Two interfaces on one word clock** more than 2 ppm off both hosts: `OwnClock`, tracking
  starts, the servo reads near 0 ppm: Measuring for 60 s, then *Not locked* as today, never
  engaged, samples untouched.
- **A locked pair with stamps worse than configured** engages only if its servo errs by over
  0.5 ppm (ten steady sd); it then runs resampled near 0 ppm, where today it would stop.
- **Beyond ±1000 ppm**: never engaged, *CLOCKS NOT LOCKED +1250 ppm* as today.

*What this costs:* where only the windows show the difference, some 20 s more than a servo
running from the stream's start. In return agreeing pairs run nothing, and no engagement rests
on a servo that has been following a clock that has since stepped.

### 4.2 Re-acquire after a rate step

A rate step (an interface relocking or falling back, A's or B's) is outside the servo's model
(a crystal drifting 0.1 ppm/h): its rate follows a step over minutes, and its stamp gate is no
detector since the wander model absorbs a ramp in part (the step test's simulation states how
long it takes). The detector is the **cross-rate windows the joiner already keeps**
(`RigState.long`, `.short`: 60 s and 40 s, the relation's own evidence). **One rule, the
joiner's, at each stamp while the servo runs:** for each ready window whose span started after
the servo's last `reset`, if |`rate_ppm` − window ppm| exceeds the relation's bound for it
(3 ppm over 40 s, 2 ppm over 60 s), then `Servo::reset`, `reacquired` + 1, and the placement is
unvouched back to that window's start (§4.4). The window's start comes from a new accessor,
`RateWindow::first() -> Option<(u64, u64)>` in `ac2-core/src/clock_rate.rs` (its oldest point:
B's index and A-local ns; A's index = ns · rate / 10⁹). Both rates are B against A, positive
B fast.

- **After a reset**, a window that spans the step counts again only once it has rolled past the
  reset: 40 s for the short window, 60 s for the long. Restarting the windows instead would
  also blank the relation's evidence. *Cost* (§9): for 40 s after a re-acquire (60 s for steps
  of 2–3 ppm) nothing catches a second step, while the transfer function already runs on a
  servo acquired after 21 s, perhaps while an interface's PLL still settles.
- **Detection time.** A step of Δ in a window moves its least-squares slope by Δ·u²(3 − 2u),
  u the share of the window since the step: the 40 s window crosses 3 ppm 21 s after a 5.6 ppm
  step, 18 s after 7.3 ppm; a 2.5 ppm step crosses the 60 s window's 2 ppm after 43 s (the
  servo following in part adds a little; to be simulated). Then ≈ 21 s of acquisition.
- **False re-acquires.** Servo rate sd 0.03–0.045 ppm; windows 0.34–0.40 ppm per 60 s
  measured (`peer_clock.rs`), about 0.62–0.73 ppm per 40 s (scaled as T^−1.5, to be
  simulated): bounds at 5–5.9 and 4.1–4.8 sd, about one false re-acquire in a hundred hours at
  the worst figure (Gaussian; steady servo; 40–60 s after a (re)start its sd is 0.5–0.67 ppm,
  a case to simulate), each 21 s of *measuring clocks…* and a stop. The 20 min fake test
  and the rig hour assert **0 re-acquired** (§10).
- A step smaller than the bounds is not caught: it reaches the phase as the filter follows it
  (under TRACKED the delay track and its drift cue show it, `remote-inputs.md` §3.2; under
  LOCKED the audio follows it within seconds).
- A pair that read Agree and goes Not locked needs nothing else: no servo ran, so tracking
  starts fresh on the relation's Not locked.

### 4.3 The relation shown

`PeerClockWatch` keeps its judging and its 60 s Not-locked hold unchanged, on a field of its
own (`held`, what `update`'s hold reads); the published `clock.relation` is then
`tracked_relation(held, sent, track)`, with `Evidence` gaining `track: Option<&TrackReport>`
and `sent: bool` (`Track` sent for the current stream). `judge` is unchanged; `last_change`
follows the published relation, the one the operator sees. In order, first match ("deciding":
acquired, not engaged, 3·`rate_sd_ppm` > 0.5 ppm, |rate| < 1000 ppm):

| held / servo | shown | cross-rig transfer function |
|---|---|---|
| *Can't tell* (any reason), or Not locked `OtherRate` | held | stops |
| `Track` not sent | held | as today |
| engaged, `ServoReport.since_stamp_s` > 60 s | **Can't tell** `StampsStopped { rig }` | stops |
| engaged, acquiring after a `Phase` reset < 10 s ago | **Tracked** (its gap columns restart the averages, as a gap does for Agree) | runs |
| `Track` sent; the servo not yet reported, acquiring or deciding; < 60 s since the `Full` reset | **Measuring** (*measuring clocks…*) | waits |
| not engaged | held | as held |
| engaged, acquiring (≥ 10 s after `Phase`, ≥ 60 s after `Full`) | **Can't tell** `NotAcquired { rig }` | stops |
| engaged, \|`rate_ppm`\| ≥ 1000 | held: the windows read Not locked `CrossRate` at that rate, so it stops | stops |
| engaged | **Tracked** (stage 2: **Locked** by §6) | runs |

- *Measuring* before engagement is `PeerRelation::Measuring` on the wire: one message, not
  *CLOCKS NOT LOCKED* then TRACKED. Past 60 s a servo still acquiring or deciding shows the held
  relation (a 1 ppm pair: Not locked until about 116 s, then TRACKED; one change). A GAP on a
  started, never-engaged pair changes nothing shown: the 60 s count from the `Full` reset.
- An engaged rig falls back to held only past 1000 ppm (Not locked there), never while it
  cannot place B: held may read Agree by then (an interface relocked), and Agree would run
  the job over a column of gaps.
- `StampsStopped { rig }` names whose stamps: B when its frames come without
  `capture_system_ns`, A (this rig) when A's stamp line is missing (`ServoReport.stamps_from`);
  an unsynced system clock is its own Can't tell, held, as today.
- **Coasting** keeps Tracked up to 60 s (design value: 60 s moved the phase 0.5–5 µs
  simulated, 10 µs on real stamps). A rate re-acquire (`Full`) reads Measuring ≈ 21 s; a phase
  re-acquire (2–3 s) stays Tracked.

### 4.4 The gate

`control/cross_rig.rs::gate_of` runs a cross-rig transfer function on **Agree** (with
`Anchor::SystemTime`, as today), **Tracked** and **Locked**. `GateBasis` gains
**`unvouched_s: Option<f64>`**, `Some` for an engaged rig; every stop of an engaged rig reaches
back by it plus the lag (`GATE_LAG_S` + link offset), in place of the reason's own span. The
joiner keeps `vouched`, an A sample index: each stamp fed while the servo is acquired advances
it to that stamp's time; a re-acquire sets it back to the start of the window that tripped;
`ServoReport.unvouched_s` = (A's newest − `vouched`) / rate. **The control thread freezes it**
at the transition into a stop (per rig, cleared when the relation runs again): the gate's
`window_s` then stays equal from report to report, so `sent == Some(gate)` dedupes and the job
gets one `JobCmd::Clocks(Stop)`, not one a second. Coasting into Can't tell reaches back past
the last stamp, a re-acquire past the step, each ≤ 60 + 1 + 2.5 s, within the 80 s the 20 s
snapshots reach (`jobs/cross_rig.rs`, five kept). The job keeps the earliest cutoff of
consecutive stops (`set_gate`), so a later stop with a shorter reach loses nothing.

**Phase under TRACKED.** Magnitude and coherence are unaffected. Over minutes the phase errs
by the stamps' slow wander, estimated 20–25 µs sd (§2): 7–9° at 1 kHz, 72–90° at 10 kHz.
Within one 8 s average the servo's fast error is 2 µs or less, so an average is not smeared;
one minute against the next, or a stored trace against a live one, moves. The **marking is the
word TRACKED** (decision 3), the phase drawn normally; the Connection page and `ac2 status` give
*phase over minutes about 30 µs rms (108° at 10 kHz)*: the servo's figure, the larger of
`phase_sd_s` (from the configured 33 µs wander) and the measured `stamp_scatter_s`, so it reads
nearer 30 µs than §2's 20–25 µs estimate, the configuration being set from the larger
measured wanders; stage 1 reads both on the rig and the words carry what the servo reports.
Fading the phase is open (§12).

**The delay track under TRACKED.** The tracker's 0.1-sample fine band is below what the
stamp wander moves in seconds, so each frequent move (`ControlMsg::DelayTracked`) logs at debug,
not info, for a tracked input.

## 5. Audio trim (stage 2)

### 5.1 What the job exports

The servo needs, per observation, the **absolute** delay between its output and the stream,
an uncertainty, its media time and a restart flag (`Servo::observe_audio`). The job's `Finder`
runs the `DelayStream` once per window — 0.25 s in the full band (`track_band`), 0.5 s in Mid,
4 s in Sub — and hands each `FinderResult` to `Tracker::observe`. The tracker is a quantizer
(it holds within 0.1 sample, `FINE_AGREEMENT_SAMPLES`, and reports only at the band's edges),
so the observation is the per-window `Arrival.delay_frac` (the lobe fit against a 1/16-sample
pulse model); its `Arrival.uncertainty` is a heuristic, not a 1-σ (0.35 × pulse width × noise
ratio, floored at 0.1 sample).

**One observation a second.** The servo was validated fed one observation a second, and a
path's jitter is not independent between windows inside one second (draughts and temperature
move a path over a second and more): fed four a second, the filter would count one path error
four times. So the job forms **one observation per second** of A's samples from the accepted
windows whose centres fall in it: `delay` their mean `delay_frac`; `sigma` max(sd / √n,
0.1 sample / √n) for n ≥ 2, the window's own `Arrival.uncertainty` for n = 1, until the rig
log below calibrates it; `at` the mean of their centres. In Sub (4 s windows) that is one
observation per 4 s.

```rust
pub(crate) struct TrimObs {
    pub(crate) rig: RigId,
    pub(crate) meas: MeasId,
    pub(crate) epoch: SessionEpoch,
    /// The observation's time, A's sample index.
    pub(crate) at_sample: u64,
    /// Positive: the remote (resampled) stream lags, seconds.
    pub(crate) delay_s: f64,
    pub(crate) sigma_s: f64,
    /// The finder's history restarted (`Finder::restart`: a stream break) since the
    /// last observation; the joiner also sets it on a feeder switch.
    pub(crate) restart: bool,
}
```

Channel: a `std::sync::mpsc::sync_channel(16)` (design value) made by `Joiner::new`; the
joiner keeps the receiver, the session keeps the sender on the control thread and clones it
into jobs. Jobs `try_send` (a full queue drops the observation; a job never blocks). The joiner
drains it with the link frames and calls `observe_audio` (media time formed as in §2).

**Sign.** The finder's convention is positive = measurement late (`meas[i] ≈ (h ∗
ref)[i − D]`); `observe_audio` wants positive = the servo's output lags. With the remote input
as the measurement (`RemoteSide::Measurement` from `cross_side`), `delay_s = +D / fs`; as the
reference, `delay_s = −D / fs`. The job is told the side with its sender
(`JobCmd::Trim(Some(TrimFeed { side, tx }))`, sent by the control thread when the rig engages;
`JobCmd::Trim(None)` when its stream ends).

**When a window counts** (design values, set from the rig log): the outcome is `Accepted` and
its first arrival is the one the tracker follows; the window lies wholly after the last
averages restart and after the gate last opened; and the excitation is broad,
`Confidence.excited_fraction` ≥ 0.8 and `pulse_width` ≤ 1.5 × `nominal_width` (a dispersive
path's first arrival is timed differently under narrowband programme; pink noise passes, much
music will not, which costs only LOCKED during it). A job with a trim sender runs its
`DelayStream` with tracking off too (`Finder::push` otherwise returns early); its tracker then
never moves the delay.

**Is the per-window delay good enough?** Unmeasured. Stage 2 starts with (1) a rig log of
per-window `delay_frac` and `uncertainty` on a locked pair (pupu loopback against ketunkolo's
mic, pink noise, an hour) against the 5 µs path jitter the servo assumes, and (2) the servo
simulation re-run with observations so formed, which sets §6's bound. If (1) shows well above
5 µs, the finer estimator is new math in `ac2-core::delay`: the change of the cross-spectrum's
phase per bin since the bias was anchored, fitted as a delay, weighted by γ² / (1 − γ²).

### 5.2 Who feeds

The servo has one audio bias per rig (`b`); two transfer functions on one remote rig measure
two paths, and interleaving them would trip the innovation gate at every switch. So **one
feeder per remote rig**, chosen by the joiner: it keeps the current feeder until that one has
sent nothing for 10 s (design value), then takes the lowest `MeasId` that sent within 10 s; the
others' observations are dropped, and the new feeder's first is marked `restart`. A transfer
function with both channels on that rig gets no sender (its delay says nothing of the clocks).

### 5.3 Who owns what

The servo absorbs the clock, B against A; the transfer function's tracker follows the delay on
the servo's output, the path (its slow part in `b`) plus the servo's residual. They do not
fight: the servo sees the finder's absolute per-window results, never the MTW's delay.
`JobCmd::SetDelay` only moves the finder's held delay (`Finder::set_held`), so an operator's
set delay changes nothing it sees; only a stream break restarts the finder (`TrimObs.restart`).
The delay drift cue (`remote-inputs.md` §3.2) keeps its words, *path or clocks*.

## 6. LOCKED, defined (stage 2)

**What LOCKED claims**: *audio and stamps are consistent*. The delay measured on the signal of
a running transfer function has matched the clock model within 10 µs rms over at least 30
observations since the model last anchored on it, none refused. It does not claim the path is
still (motion slower than the bias walk is absorbed into `b`, the delay where it belongs), nor
anything of the interfaces' clock sources. The operator (2026-11-25): once the audio is trimmed
on signal, "locked" is a fair observation. `Status::audio_sd_s` is not that evidence (it reads
about 2 µs whatever the path does), so stage 2 adds to `ac2-resampler`:

- **`Status::audio_used: u32`**: observations taken in `observe_audio`'s `Used` branch since
  the bias was last set;
- **`Status::audio_scatter_s: Option<f64>`**: the rms of their innovations, a plain sum of
  squares and count fed in the `Used` branch and cleared at every `BiasReset` (`reset`,
  `reset_phase`, a `restart`, a refusal). Not a ring, not exponential: it moves slowly once it
  has 30 behind it.

**One rule** (control thread): **Locked** ⇔ the row would read Tracked, `audio_used` ≥ 30,
`audio_scatter_s` ≤ 10 µs and `audio_sd_s` ≤ 10 µs (design values; 36° at 10 kHz). Its
negation is the leave rule; there is no second bound.

- **Against the gate.** The innovation's predicted sd is about 5.5 µs at the 5 µs path
  jitter, so the 5 sd `audio_gate` lies near 27 µs: a path well above 10 µs is refused now and
  then, each refusal resets the bias and `audio_used`, and LOCKED waits for 30 more.
- **Near the bound the word alternates** (one rule, no hysteresis): at 8–10 µs true scatter a
  refusal every few minutes (≈ 140 s at 10 µs), each a half-minute of TRACKED; at count 30 the
  estimate has about 1.3 µs sd. A leave bound would not stop the refusals; so none.
- **Silence.** The scatter freezes and `audio_sd_s` grows from about 2 µs by the rate's
  uncertainty (3.3–5 µs over 110 s) and `b`'s walk (8.7 µs over 110 s), passing 10 µs after
  about **1 min 50 s** (stage 2's simulation states it): a pause between songs keeps LOCKED.
  `audio_hold_s` plays no part (`ServoConfig` is fixed at `Servo::new`).
- **Sub** (one observation per 4 s): two minutes of signal; its coarser timing may keep it
  TRACKED.

**Which panes.** LOCKED is shown for the rig and on the feeding transfer function's pane only.
Another transfer function on the same rig reads TRACKED: its path is not the one trimmed, and
§9's leak can put up to 58 µs of the feeder's path into its clock.

**Path jitter.** `path_jitter_s` is 5 µs (indoor, up to 20 m); outdoors 25–50 µs (servo docs).
Fixed indoors in this stage (decision 2: no setting); outdoors the refusals keep TRACKED. Open
(§12).

## 7. What is shown

Words in `ac2-scene` first, asserted as scene text; short per decision 3, figures only on the
Connection page and in `ac2 status`.

- **The rig's heading** (`cross_rig.rs::clock_words`): *TRACKED +7.3 ppm*, *LOCKED +7.3 ppm*;
  neither a warning. The ppm is the servo's rate, one decimal.
- **Pane status line** (`caption`): *TRACKED +7.3 ppm · cm4*, *LOCKED +7.3 ppm · cm4* (the
  feeder only, §6), in place of *clocks agree · cm4*.
- **Connection › Between rigs** (`between_rigs.rs::relation_with_evidence`):
  *TRACKED +7.3 ppm: cm4 resampled onto pupu's clock from capture stamps; phase over minutes
  about 30 µs rms (108° at 10 kHz)* /
  *LOCKED +7.3 ppm: trimmed on the signal of TF 3, within 6 µs rms of the clock model; phase
  over minutes from capture stamps* /
  *TRACKED +7.3 ppm: no capture stamps for 12 s, holding the rate* (coasting) /
  *measuring clocks… (the clock servo acquiring cm4's clock)* (acquiring or deciding); and on
  the rig's block *0 re-acquired*. The cross-rig rate windows' line stays below it.
- **Clock domains** (`clock_domain.rs`): a tracked rig is its own domain; `pair` maps both
  relations to a new `Differ::Tracked { ppm }` (*resampled onto pupu's clock, +7.3 ppm*).
  **The word LOCKED** (recommended, put to the operator, unanswered): `tag_of` reads `TRACKED`
  and `LOCKED`, the same words on every surface; remote-auto §10 q1's reservation (no drift
  ≥ 30 min; no tag ever read it) is dropped, in `clock_domain.rs`'s module doc and there. The
  other reading is §12 q5. `add_beside` already adds a differing standing line (*FREE*) to the
  tip and keeps the stream's tag: no new rule.
- **`ac2 status`**: *between rigs cm4 · TRACKED +7.3 ppm* and the Connection page's line
  indented under it.
- **Can't tell**: *cm4's capture stamps stopped* / *pupu's capture stamps stopped*
  (`StampsStopped`), *cm4's clock not acquired from its stamps in 60 s* (`NotAcquired`).
- **Servo started, not engaged** (deciding past 60 s, or settled under the floor): the held
  words (*CLOCKS NOT LOCKED +3.1 ppm*, `OwnClock`) as today; the Connection line adds *clock
  servo: cm4 +0.1 ppm against pupu, not tracked*.

**Gone for a tracked pair:** *CLOCKS NOT LOCKED +7.3 ppm* on the heading, the pane's *Clocks
not locked: averaging stopped*, and the slip cues; all stay for pairs that are not tracked.

**Every exhaustive `match` on `PeerRelation`** gains the arms (they fail to compile otherwise):
`ac2d/src/control/cross_rig.rs::gate_of`; `ac2-scene/src/clock_domain.rs` `tag_of` and `pair`;
`ac2-scene/src/between_rigs.rs::relation_text`; `ac2-scene/src/cross_rig.rs::clock_words`.
Wildcard matches that need a deliberate look: `between_rigs.rs::relation_with_evidence`,
`cross_rig.rs`'s stop words (`(StopWhy::…, _)`), `remote/standing_verdict.rs` `Raw::of` and
`quantise` (the standing feed never produces either), `remote/peer_clock.rs::update`. Samples:
`ac2-proto/src/samples/remote.rs` gains a tracked rig.

## 8. Wire, files, versions

- **Stage 1, the next free `PROTO_VERSION`:** `AudioHeader.capture_system_ns`;
  `PeerRelation::Tracked`; `CantTellReason::StampsStopped { rig }` and `NotAcquired { rig }`;
  `PeerClock.tracking: Option<Tracking>`, `Some` once `Track` started the servo:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tracking {
    pub mode: TrackingMode,
    /// Per rig: the servo is the rig's.
    pub reacquired: u32,
}

/// Rates are the servo's, B against A, ppm; `phase_sd` the larger of its `phase_sd_s` and
/// `stamp_scatter_s`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TrackingMode {
    Acquiring,
    /// Acquired; the rate not yet clear of the floor.
    Deciding { rate_ppm: f64 },
    /// Acquired, not engaged: under the floor, or beyond the resampler's range.
    NotTracked { rate_ppm: f64 },
    /// Engaged, the rate beyond the resampler's ±1000 ppm: steps are clamped.
    Saturated { rate_ppm: f64 },
    Tracking { rate_ppm: f64, phase_sd: Seconds },
    Coasting { rate_ppm: f64, phase_sd: Seconds, since_stamp: Seconds },
}
```

  (`PeerClock.ppm` keeps the windows' figure, hence `rate_ppm`.) The `PeerRelation` doc line
  *Never locked* (`ac2-proto/src/model/remote.rs`) is rewritten to say what LOCKED claims.
- **Stage 2, the next free `PROTO_VERSION` again:** `PeerRelation::Locked`; `Tracking` gains
  `trim: Option<Trim>` (`None`: no feeder), `Trim { by: MeasId, scatter: Seconds, used: u32,
  refused: u64 }`, same derives.
- With each: `docs/protocol.md`, `fixtures/protocol` and `WIRE_LOCK`, the Python
  cross-language fixtures (`tools/protocol`), and the ac2-remote viewer's pin. `TrackReport`,
  `FanoutMsg::Track`, `TrimObs` and `JobCmd::Trim` are in-process only.
- **`ac2-resampler`**: `Status::rate_sd_ppm` (stage 1); `audio_used`, `audio_scatter_s`
  (stage 2).
- **Session format, rig file: unchanged.** Nothing is set.
- **Recordings** of a remote input are of the joined feed. The raw sidecar (`ac2-traces::raw`,
  `VERSION` 3 → 4) gains `remote_timing: Vec<RemoteTiming>` beside `remote_slips`, truncated
  the same way:

```rust
pub struct RemoteTiming {
    /// File channel (zero-based).
    pub channel: u16,
    /// Audio file frame from which it holds.
    pub frame: u64,
    pub session_sample: SampleIndex,
    pub placed: RemotePlacement,
}

#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemotePlacement {
    /// Integer k: the source's samples as sent.
    Exact,
    /// Resampled onto this rig's clock; the servo's rate then, ppm.
    Tracked { ppm: f64 },
}
```

  `jobs/record.rs::note_remote` writes an entry at the recording's first block for each
  remote channel (`Exact` when it has no `tracked` mark), when `tracked` turns on or off, and
  when its value changes after a whole-block gap (a `Full` re-acquire; a phase re-acquire or
  link loss keeps the rate, so writes nothing); the gaps are `RemoteGap`s as any.

## 9. Not claimed

- **The constant offset** between the rigs (path, interface latencies, §2's 3.6 µs): unmeasured,
  as today; the delay readout keeps saying it includes it (operator: no calibration step).
- **Phase above a few kHz over minutes on stamps alone** (§4.4): an estimated 20–25 µs sd.
  LOCKED improves the short term only; the long term is still the stamps'.
- **Path changes faster than the crossover.** The servo cannot tell a path change from clock:
  of a 160 µs change (1 °C over 30 m), 58 µs goes into the clock estimate over a minute, 31 µs
  over twenty (servo tests). The feeder still measures the whole change; anything *else* timed
  against that stream is misled by up to that share, hence §6's TRACKED on other panes.
- **Rate steps smaller than the relation's bounds** under TRACKED, and **a second step within
  40 s of a re-acquire** (60 s for 2–3 ppm), when the servo may have acquired on a PLL still
  settling (§4.2).
- **Mixed sample rates.** The resampler covers ±0.1 %; another nominal rate stays refused.
- **A transfer function across two different remote rigs** stays refused: no trim could be
  attributed, and its phase would carry both rigs' wander. A later stage.
- **Outdoors**, the indoor path jitter (§6).

## 10. Tests

- **Fake rigs** (`ac2-audio` fake). Today the fake is paced at the nominal rate while its
  stamps follow `true_ns`: at +7 ppm its capture falls 8.4 ms behind its delivery over 20 min,
  late frames a real device never makes. Stage 1 adds:
  - **pacing by `true_ns(frames)`** in `Pace::Realtime`; `true_ns` integrates
    `audio_clock_ppm` plus each `ClockStep` in the fault list from its block on, so it stays a
    pure function of the config, shared by the stamp clock;
  - a **shared signal of true time**, `InputContent::TrueTimeNoise { delay_ns: i64 }`
    (integer, as `InputContent` derives `Eq`): white noise band-limited to 0.45 × rate, defined
    on a grid at 2 × rate by a counter hash of a fixed seed and the grid index, and evaluated
    at each sample's true time (`system_origin_ns` + its `true_ns` − a process-wide epoch, the
    first fake's, in a `OnceLock` − `delay_ns`) through a 64-tap Kaiser-windowed sinc (β 8.6,
    cutoff 0.45 × rate) from a 1024-phase table, adjacent phases interpolated linearly (128
    multiply-adds a sample), grid values cached in a ring. Band-limited, so it can be evaluated
    at any instant (raw noise could not), dense in frequency where 200 log-spaced tones would
    leave the finder's bins unexcited (`min_excited_fraction`), and cheap where a multitone
    would not hold real time at `ac2-audio`'s dev opt-level. A test in `ac2d/tests/it` runs it
    through `DelayStream` first and asserts `Accepted` with `excited_fraction` ≥ 0.8, which
    stage 2's tests need too. Two in-process daemons' fakes share it, and a +7 ppm device
    drifts against a 0 ppm one analytically;
  - `FakeFault::NoCaptureStamps { at_block, blocks }` and `FakeFault::ClockStep { at_block,
    ppb: i64 }` (integer: `FakeFault` derives `Eq`); `at_block` is the first frame of that
    callback, frames an `Xrun` lost counted. Stamp wander waits for stage 2.
- **Daemon, two in-process daemons on fake devices** (`ac2d/tests/it/remote_inputs.rs`).
  Rates sit well clear of every bound (the fakes' stamps are exact):
  - +7 ppm free-running source, its session running 70 s before A takes the input (own line
    `Measured`): TRACKED within 35 s of the stream, the transfer function runs, its delay
    within ±0.5 sample of its first locked value over 20 min (exact stamps leave the servo
    under 0.1 sample), with no slip, no late frame and **0 re-acquired** (`slow_`);
  - only the cross rate apart (own lines +1.2 and −1.2 ppm, 2.4 ppm between them): Measuring
    from the 60 s window on, never Not locked shown, TRACKED within 120 s;
  - two fake interfaces on one clock 3 ppm off both system clocks: Measuring for 60 s, then
    Not locked (`OwnClock`) as today, servo started, **not** engaged;
  - an agreeing pair stays Agree, no servo started, its joined feed **bit-identical** to the
    source's samples;
  - `NoCaptureStamps` for 70 s: Coasting, Tracked for 60 s, then Can't tell (*capture stamps
    stopped*), the held result from before the last stamp; stamps back: Tracked;
  - a source restart: a new stream, Measuring, engaged again;
  - a source gap (`GAP`): `reset_phase`, a 2–3 s column gap, the relation Tracked throughout
    and no stop, resampled output again within 4 s; a break on A:
    every servo `reset_phase`s;
  - `ClockStep` +5.6 ppm under tracking: re-acquired once, *measuring clocks…*, Tracked again
    within 50 s (21 s detection + 21 s acquisition + ticks), the held result from before the
    step (the stop reaching back to the tripped window's start);
  - a pair that agreed, then `ClockStep` +5.6 ppm: stopped and held as today, then tracking
    started and TRACKED;
  - a fake at +1500 ppm, same nominal rate: not engaged, *CLOCKS NOT LOCKED +1500 ppm*;
  - stage 2: pink-noise transfer function → LOCKED after 30 observations; generator off →
    Tracked after about 1 min 50 s; white path jitter of 30 µs → refusals, stays TRACKED;
    **correlated** path jitter (Gauss–Markov, 20 µs sd, τ 5 s) → stays TRACKED, its
    `audio_scatter` reading it; a step in the path delay → a bias reset, no phase step,
    LOCKED again after 30 s; two transfer functions on one rig: one feeder, the other's pane
    TRACKED, the feed kept while the feeder has signal and moving with `restart` 10 s after it
    stops; a remote *reference*: LOCKED reached (the sign).
- **Joiner unit tests** (`join_tests.rs`): one stamp a second, the mean of its frames, with
  **unequal periods** (B 256, A 1024 and the reverse) and a bunched burst; a second under 90 %
  or cut by a reset not fed; a stale or duplicate `Track` ignored; a stream bump re-anchoring
  every input of the rig; link-side loss resets one resampler, not the servo; a reset's head a
  gap without warning; late from `available < len`, margin from the pushed index; no slip; the
  hand-over block all gap, `tracked` from it on; the re-acquire rule; `vouched`; the
  link default.
- **Control unit tests** (`cross_rig_tests.rs`): `tracked_relation` row by row (Measuring in
  the publish that sends `Track`; Tracked through a `Phase` reset); `gate_of` with
  `unvouched_s` frozen (one `JobCmd::Clocks` per stop); `track_sent` cleared with the joiner.
- **Recorder:** a recording started after TRACKED opens with a `Tracked` entry.
- **`ac2-resampler`**: `rate_sd_ppm` (stage 1); `audio_scatter_s` reads injected path jitter,
  not the model's figure; it and `audio_used` clear on every bias reset (stage 2).
- **Scene:** every string of §7; the clock-domain tags `TRACKED` and `LOCKED`; the slip
  and *CLOCKS NOT LOCKED* strings absent for a tracked pair.
- **Rig acceptance (operator present; it emits):** pupu's loopback against the CM4's USB mic,
  pink noise: TRACKED within 30 s; LOCKED within a minute of noise; delay steady and **0
  re-acquired** over an hour; the phase compared at 1 and 10 kHz minute to minute (the 20–25 µs
  estimate checked); acquisition time and stamp scatter at §2's configuration; noise off,
  Tracked after about two minutes. Before stage 2: §5.1's per-window delay log.

## 11. Stages

Each lands green on its own.

- **1 — tracked on stamps (daemon).** `capture_system_ns` on the wire; the fake's pacing,
  shared signal and two faults; `Status::rate_sd_ppm`; the servo, one stamp a
  second, the resamplers, `FanoutMsg::Track`, `TrackReport`, engage, the tracked path, the
  re-acquire, `vouched`, `RateWindow::first`; `tracked_relation`, `Tracked`, `StampsStopped`,
  `NotAcquired`; `GateBasis.unvouched_s`;
  the link default; the sidecar's `remote_timing`; scene words, clock domains and `ac2
  status`. Testable alone: everything but LOCKED. Delivers the operator's case at stamps-only
  phase quality.
- **2 — audio trim and LOCKED.** Starts with the rig log and the simulation (§5.1); then
  `audio_used` and `audio_scatter_s` in `ac2-resampler`, the per-second observations, `TrimObs`,
  the feeder, LOCKED and its words. Needs stage 1's tracked path.
- **3 — the app.** The heading, the caption and the Connection page's lines placed; snapshot
  for the final look.
- **Later:** a stated absolute offset between rigs (timing study B2/B3); a transfer function
  across two remote rigs (§9); outdoor path jitter (§12).

## 12. Open questions

1. **Fade the phase under TRACKED?** Above about 1.1–1.4 kHz the estimated wander exceeds 10°
   over minutes. *Recommendation:* no; a faint phase reads as *held result* (decision 8b).
2. **LOCKED's bound**, 10 µs rms, re-checked against the rig log. *Recommendation:* one bound;
   a gusty or narrowband case reads TRACKED.
3. **Outdoor path jitter.** *Recommendation:* indoor value now; later estimated from the audio
   innovations (the figure LOCKED reads), so no setting is ever needed.
4. **Stored traces under TRACKED** carry phase good to about 30 µs rms against a later live
   one. *Recommendation:* not in these stages; later record the relation with the trace.
5. **LOCKED names two claims.** `remote-auto.md` §10 q1 (operator, 2026-11-19) reserves
   `LOCKED` for rigs shown to hold no drift ≥ 30 min: evidence of one shared clock. Here
   *LOCKED +7.3 ppm* names a rig whose clock demonstrably differs and is corrected. §7 takes
   the recommended reading (today's meaning everywhere, the reservation dropped). *The
   alternative:* `tag_of` keeps `TRACKED` for both relations, the reservation stays, and the
   trimmed state gets another word (*TRIMMED*) on the heading, caption, Connection page and
   `ac2 status`; nothing else changes. Awaiting the operator.

## 13. Decisions

Operator, 2026-11-25:
- **Resample on the taking rig**, in its joined feed; the source rig is unchanged.
- **Automatic**: tracking only when the stamp relation reads Not locked by rate with both
  system clocks synced and capture stamps present (system-time anchor); an agreeing pair stays
  bit-exact; once engaged, engaged until the stream restarts or re-anchors.
- **Short words**: *TRACKED* and *LOCKED* with the ppm; figures on the Connection page and in
  `ac2 status` only.
- **No calibration step**: the constant offset stays unmeasured.
- **Audio trim is in the workflow**: the normal measurement's signal trims; nothing extra is
  played.
- **Can't tell stays refused**: no servo without synced clocks and capture stamps.
