# Timing study: capture time across rigs (multi-rig stage 3 precondition)

Status: plan, not run. Precondition named in `multi-rig.md` §3; results go to
`multi-device.md` §7 and decide the open items of `multi-rig.md` §3.

Stage 3 lets one rig's measurement use an input captured by another rig. To combine the two
streams sample-accurately, A must know which of its own sample instants sample *n* of B's input
belongs to: a **map from B's sample index to A's**, as an offset plus a ratio. This study
measures how well each available source of that map knows it, on the rigs in use, before any
design promises a number.

## 1. Questions

| # | Question | Decides |
|---|---|---|
| Q1 | How much does a backend capture stamp scatter about a straight line in sample index, on the raw stamp clock, per host? | the stamp filter; how long until an offset settles |
| Q2 | How does the rate ratio between two rigs move over an hour? | the converter's ratio tracking (§5 of `multi-device.md` gives the model) |
| Q3 | What is the offset between a stamp and the instant the ADC sampled, per interface, rate, buffer size and input type? Does the backend's reported latency cover it? | whether stamps alone can give a cross-rig delay |
| Q4 | Is that offset the same after a stream restart, a jackd restart and a reboot? | per-interface constant, or measured on each open |
| Q5 | How do the two hosts' disciplined clocks differ, and how steady is that? (Absolute error only as chrony reports it; nothing here measures the true second.) | the host term of the stamp path |
| Q6 | How do block-sized messages arrive over the existing data connection under normal load? | the buffer delay (*remote inputs add N ms*) |
| Q7 | Measured directly: how does B's sample index map onto A's, from one channel captured by both? | the reference every other path is judged against |

## 2. Error budget

The stage 3 offset can come from three sources, with different budgets. Target: **±10 µs**
(3.6° at 1 kHz, 36° at 10 kHz; half a sample at 48 kHz is 10.4 µs; both rigs run 96 kHz,
where half a sample is 5.2 µs).

| Term | Path | Expected | Measured by |
|---|---|---|---|
| Stamp residual after the line fit | stamps | µs on USB; larger on pupu, where FireWire iso cycles quantise period timing at 125 µs before JACK's DLL | phase A, Q1 |
| ADC-to-stamp constant (converter group delay + whatever the reported latency misses) | stamps | tens of samples, rate dependent; RME manuals state AD latency per rate | phase B, Q3 |
| Host clocks against each other | stamps | **tens to hundreds of µs.** Both hosts run *software-timestamped* PTP: ketunkolo over a veth on a bridge, pupu unicast across a router (~0.5 ms path, asymmetry unmeasured). The ±10 µs target does not survive this term. | phase B, as a cross-host composite; absolute error only as chrony self-reports |
| Network arrival jitter | all | ms; adds latency, not offset error | phase C, Q6 |
| Shared digital channel into both rigs | Orion ADAT | exact: an integer number of samples | phase B, Q7 |

The consequence is stated up front: on these hosts as they are, **the stamp path gives a delay
to tens of µs at best**, which is enough to align a remote input within a sample at 48 kHz only
if the host term is far better than its paths suggest. A shared channel reaches the target
without any system clock. Phase B measures both, so the gap is a number rather than
this estimate.

## 3. What exists today

- `BlockStamp.capture_ns` (`crates/ac2-audio/src/block.rs`): JACK fills it with the engine's
  DLL-filtered cycle start minus the port's capture latency range *max* (`jack_host.rs`); ASIO
  from `systemTime` when flagged valid; Core Audio from `mHostTime`. The daemon does not use it
  yet; only `ac2 selftest` reads it (`ac2-cli/src/selftest/stats.rs`).
- On ketunkolo jackd's clock is CLOCK_MONOTONIC_RAW (measured 2026-11-06); pupu's is unverified.
  Stage 2's fix (454bc168) pairs the stamp clock with CLOCK_MONOTONIC by bracketed reads; the
  logger adds CLOCK_REALTIME.
- The data wire between daemon and clients is ROUTER/DEALER with per-peer byte credit
  (`flow-control.md`); `multi-rig.md` §3 still says PUB and is corrected when stage 3 is
  redesigned.
- Ground truth (`~/src/sys/aika.md`): **aika**, GNSS time server with an AR-40A rubidium, PTP
  grandmaster of both hosts, u-blox ZED-F9T 1PPS (not used: no PPS to the rigs, §5 phase B). ketunkolo's Babyface Pro (96 kHz) is clocked
  over ADAT from the same reference (≈0 ppm). pupu's FF400 (96 kHz/256, read from the logger 2026-11-09) is free-running (≈ +5.6 ppm).

## 4. Standards: what they answer and what they don't

| Standard | What it fixes | Use for ac2 |
|---|---|---|
| **AES67** with **RFC 7273** | Sender media clocks locked to PTP. The RTP timestamp is the media-clock count modulo 2³², offset from the PTP epoch by `a=mediaclk:direct=<offset>` (zero by convention); `a=ts-refclk` names the PTP clock. Receivers play out at a fixed **link offset**; the standard sets a minimum receive buffer. | The time model for stage 3's stream: each block carries B's sample index and B's best statement of its instant; A plays it in at a fixed link offset (the buffer policy). Not the transport. A native AES67 backend stays postponed (no gear). |
| **IEEE 1588** PTP; the **AES67 media profile** and **SMPTE ST 2059-2** are two distinct profiles (domains, message rates) | Host clock agreement to sub-µs **with hardware timestamps** | Neither rig host has a PHC; the profile tells what a hardware-timestamped setup would give, not what these hosts give (§2). |
| **SMPTE ST 2110-10 / -30** | -10 holds the timestamp semantics; -30 the AES67 conformance levels by packet time and channel count | Packet-size reference if ac2 ever speaks RTP. |
| **IEEE 1722 / 802.1AS** (AVB, Milan) | Presentation time = capture time + max transit (2 ms for Class A); clock reference (CRF) streams | Same lesson as the link offset: a fixed, stated delay. |
| **AES11** (DARS) | Frequency grades (Grade 1 ±1 ppm, Grade 2 ±10 ppm); output frame phase within ±5 % of a frame of the reference | Locking every converter to one reference fixes the **ratio**, not the offset: sample index A↔B is still an unknown constant per open. |

What none of them settle: **converter latency.** AES67 sets no tolerance between a sender's
ADC instant and its RTP timestamp; Dante and RAVENNA devices publish per-device input
latencies, and a USB/FireWire interface behind JACK reports a nominal one nobody checked. That
is Q3, for every kind of device.

### Stage 3 modes, by where the map comes from

The rigs run at 48 and 96 kHz, so a rate converter is needed in every mode; what changes is
whether its ratio and its offset are *known* or *tracked*:

| Offset from | Ratio | What stage 3 can state |
|---|---|---|
| A shared channel (one Orion source on both rigs' ADAT inputs) | exact (both locked to the Orion) | sample-accurate delay; the target is reachable |
| Stamps + disciplined host clocks | locked: exact (1 : 1 at the rigs' common 96 kHz; a fixed rational converter at different rates, never an ASRC); free-running: tracked from stamps | delay within the measured budget of §2, stated with it |
| Neither | tracked | *offset unknown*: magnitude only, phase marked (`multi-rig.md` §4) |

If Q4 shows the USB latency moves per open (Babyface-class devices can shift by USB frames on
stream start), the practical stage 3 answer is the shared channel, and that changes what stage 3
must build.

## 5. Phases

Nothing in this study emits sound from ac2. One tool covers phases A and B. Two optional
checks put an electrical signal on an interface output through other tools; the operator runs
them: a `jack_iodelay` round trip per rig, and the analog-input constant at the end of phase B.

### Tool: `ac2 selftest stamps [--keep <channel>]` (build first)
Capture-only, no daemon: opens the backend as an extra JACK client beside the running daemon
and refuses outputs (the same rule as `selftest duplex`). Per block it writes:
- sample index, frames, flags;
- `capture_ns` on the raw stamp clock, and the port's capture latency range (min and max: the
  stamp uses max, which biases it when the range is not degenerate);
- paired CLOCK_MONOTONIC and CLOCK_REALTIME reads with the bracket width.

With `--keep`: that channel's samples are written beside the CSV unmodified (f32 WAV, which
holds 24-bit samples exactly, with the block index), for phase B's comparison. The CSV header
records the input's TotalMix state (gain, EQ, dynamics), so that a failed bit match can be
explained. Every 10 s, a side log of `chronyc tracking` and
`pmc TIME_STATUS_NP`. Analysis in Python under `tools/experiments/timing/` (like `q1/`); CSVs
and recordings from the rigs stay private.

### Phase A: stamps alone (agent-run now, both hosts, 1 h each)
The logger without `--keep`, at each rig's settings (pupu 96 k/256, ketunkolo 96 k/128),
then one other buffer size each.
- **Answers:** Q1 (residual on the raw stamp clock; REALTIME is slewed by chrony and would read
  as wander) and Q2 across hosts (both runs concurrent, ratio from the stamp lines mapped to
  REALTIME).
- **Also:** verifies pupu's jackd clock source.

### Phase B: one clock, one shared channel (operator cables; agent-run after)
Operator decision 2026-11-09: no PPS run to the rigs (possible, but a lot of work). The
**Antelope Orion 32HD** is the master clock: it takes the rubidium in and already clocks
ketunkolo's Babyface over ADAT. It gives pupu's FF400 word clock too.

- **B1: lock.** FF400 clock source **ADAT In**, from an Orion ADAT output. The ADAT frame
  clock is 48 kHz whatever the Orion runs at (96 kHz is S/MUX on ADAT), so it sidesteps the
  question of which word clock rate the Orion sends. Word clock stays the alternative, verified in the FF400 and Orion manuals before
  cabling.
  - Stage 2's clock line on pupu must then read ≈ 0 ppm like ketunkolo's: the first check,
    already in the app.
  - The ratio between the rigs is then exact, 1 : 1 at 96 kHz, so the stage 3 converter is a fixed
    decimator/interpolator.
  - The logger repeats phase A locked. With the sample clock at 0 ppm against the rubidium,
    each stamp line against REALTIME becomes a µs-class reading of that host's chrony wander
    against the rubidium.
  - With both rigs locked, the stability of the stamp path's cross-rig composite over an hour
    and across restarts (the Q4 verdict) needs no shared channel. Only its value needs B2.
- **What "locked" means here.** The reference does not clock the converter directly: the
  interface's own PLL (RME's SteadyClock on the units that have it; check the FF400 manual)
  is disciplined by the word clock or ADAT edges and makes the sample clock. Locked, the PLL
  tracks the reference's frequency with a bounded phase error: no long-term drift, a
  short-term wander inside the loop's bandwidth, and an arbitrary phase between word clock
  edge and sample index set at each lock. B1 checks what each part means for stage 3:
  - **No slips:** over an hour the locked rig's rate against REALTIME stays at 0 ppm within
    the host clock's noise (ketunkolo already reads 0 / 0 ppm).
  - **Wander:** sub-sample, below what the stamp path can see; only B2 could resolve it.
    A µs-class wander would show up as the cross-rig composite moving.
  - **Losing the reference:** the PLL holds over, and the interface falls back to its internal
    clock. Pull the word clock for a minute once: stage 2's clock line should leave 0 ppm, and
    stage 3 must notice the same and stop treating the ratio as exact.
- **B2: the offset (Q7), without emitting.** The lock fixes the ratio only: which sample of A
  goes with which sample of B is still an unknown constant per open.
  - The Orion routes one source to both ADAT outputs; even an unconnected input's own noise
    floor will do, so nothing is played. Both rigs then capture identical samples.
  - With the Orion at 96 kHz, ADAT carries each channel as S/MUX: two 48 kHz slots, even and
    odd samples. Both rigs run 96 kHz and read x[m] back from the pair, so the map is
    `pupu[n] = ket[n + d]`, an integer d. (A rig at 48 kHz would read the pair as two plain
    channels a[n] = x[2n + p], b[n] = x[2n + 1 + p]: still exact, `a[n] = ket[2n + d]`.)
  - The analysis converts both captures to integers and searches integer lags for **bit
    equality**, not float cross-correlation. Equality proves the map. A mismatch means
    something touches the samples (TotalMix gain/EQ in the record path, dither, a rate
    converter), and that is a finding in itself.
- **B3: the stamp path (Q3–Q5 composite).** With B2's exact map as truth, each rig's
  stamp-derived REALTIME for the shared sample minus the other's is the stamp path's cross-rig
  error: stamps, ADAT receive constants and the difference of the hosts' clock errors
  together. It's measured at both buffer sizes and again after a stream restart, a jackd
  restart and a reboot (Q4).
- **Priors:** the RME manuals' AD latency per rate. The operator-run `jack_iodelay` (above)
  checks `cap_lat`.

What it does not give:
- **Absolute latency.** Each interface's absolute ADC latency against the true second needs the
  PPS. Stage 3 needs only the cross-rig map, so B2 is enough.
- **Analog inputs.** B3 holds for the ADAT input. An analog input on the same interface differs
  by (ADC group delay − ADAT receive delay), a per-interface, per-rate constant. It comes from
  the RME manual, or from an operator-run electrical loop: an Orion analog output into the
  rig's line input, with the same source on ADAT.

### Phase C: network (agent-run, no audio)
A small binary against `ac2-zmq` streams block-sized messages of zeros at the real rate (96 k
mono f32 = 3.1 Mbit/s), daemon host to daemon host, over CURVE ROUTER/DEALER with credit, for
1 h while the app runs as usual.
- **Logs:** send and arrival instants and credit stalls.
- **Answers:** Q6 (p50/p99/p99.9/max). The buffer delay is the bound plus one block.

### Fallback: one electrical signal into both rigs (operator-run)
If the FF400 cannot take ADAT from the Orion, the operator splits a line signal into a line
input on each rig.
Cross-correlation gives the difference of the two interfaces' stamp errors and the map.
- **Accuracy:** 48 k against 96 k needs a resample first; tens of µs at best.
- **Why operator-run:** routing risk, not emission by ac2.

## 6. What the numbers decide

| Result | Stage 3 consequence |
|---|---|
| Q7 map found every open | the shared-channel path is the sample-accurate mode; build it first |
| Stamp composite within ±10 µs and steady across restarts | the stamp path is a number too; a per-interface constant, measured once like a mic calibration |
| Composite steady but larger | stamp path stated with its bound (*±60 µs*) |
| Composite moves per open | stamp path is *offset unknown* without the shared channel |
| Ratio wander of a free-running rig (phase A, before the lock) | converter tracking bandwidth for rigs not on the Orion, stated |
| Network p99.9 + one block | the stated *remote inputs add N ms* |

### First target: a transfer function over a remote stream

Operator 2026-11-09: the first use is a live transfer function where one of the two channels,
the reference or the measurement, arrives as a remote stream. That is the easy target:
- **The delay finder absorbs the offset.** A transfer function's delay is found by correlating
  reference and measurement, so an unknown but *constant* cross-rig offset only adds to the
  found delay. Q3/Q7 (the absolute map) is not needed. The delay readout then says it includes
  the cross-rig offset, unless B2 has measured it.
- **What it needs:**
  - **an exact ratio**, from the Orion lock (B1) or a tracked converter for a free-running rig
    (Q2): any drift tilts the phase and smears averaging (`multi-device.md` §7);
  - **an offset that stays put** within a session: no slips (B1), no stream re-anchoring;
    a jump is handled like the delay finder's own re-lock;
  - **a buffer** that covers the network (Q6).
- **What changes in `multi-rig.md` §3:** it says the reference is never remote. That holds for
  the *timing monitor's* loopback, which must return on the same converter as its output. A
  transfer function's reference channel may come from another rig, with the stimulus's own
  loopback still watched on the rig that plays it.

- **A rig at another site** (operator 2026-11-09): no shared cable, so each site disciplines
  its own interface from GNSS, through a GNSS-disciplined word clock generator or a local PTP
  grandmaster. That is how AES67/ST 2110 remote production keeps sites in one time base without
  carrying PTP over the WAN. Both rigs then read 0 ppm in stage 2's clock line, so the ratio is
  exact as with the Orion, and the offset is the delay finder's again.
- **Buffering follows AES67/Dante, not a new design.** The receiver plays a block in at its
  timestamp plus a fixed **link offset**, Dante's receiver latency setting. A block arriving
  late is a marked gap, counted, never stretched. The offset is fixed for a session: moving it
  shifts every delay, and the transfer function would re-lock. Phase C only picks the default
  link offset for a LAN, and the figure the Audio & I/O page states. A WAN rig gets a larger
  setting, chosen by the operator.

So phase A (ratio, stability) plus B1 (lock, no slips) plus C (network) are enough for the first
stage 3 build. B2 and the stamp path's value only matter for a delay stated in absolute terms.

## 7. Write-up

The results go into `multi-device.md` §7 as measured tables, with the methods above:
- host, interface, input type, rate, buffer;
- composite mean and σ;
- Q7 map residual;
- restart spread.

`multi-rig.md` §3 is then rewritten from them, PUB corrected to the data wire as built. Stage 3
goes through the design review loop before any build.

## 8. Operator decisions

2026-11-09:
- **No PPS to the rigs.** Possible, but a lot of work.
- **The Orion 32HD clocks pupu.** It takes the rubidium in, clocks ketunkolo's Babyface over
  ADAT, and can give pupu's FF400 word clock easily.
- **Word clock first.** ADAT from the Orion to the FF400 is possible in theory, but there are
  no spare fibres. So phase B1 uses word clock (rates checked in the FF400 and Orion manuals
  before cabling).
- **B2 (shared ADAT channel) only if no other option** gives the cross-rig map. Until then
  the stamp path (B3) stands without an exact truth: its stability across restarts (B1's yield)
  is measured, its absolute value is not.
- **Stage 3's stream is ac2's own,** over the data wire it already has, using the AES67 time
  model: sample index plus its instant, played in at a fixed link offset. Real AES67 support
  is the future direction (send and receive), not stage 3.
