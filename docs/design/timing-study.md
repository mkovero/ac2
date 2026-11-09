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
| Q5 | How far is each host's disciplined clock from the true second? | the host term of the stamp path |
| Q6 | How do block-sized messages arrive over the existing data connection under normal load? | the buffer delay (*remote inputs add N ms*) |
| Q7 | Measured directly: how does B's sample index map onto A's, from one channel captured by both? | the reference every other path is judged against |

## 2. Error budget

The stage 3 offset can come from three sources, with different budgets. Target: **±10 µs**
(3.6° at 1 kHz, 36° at 10 kHz; half a sample at 48 kHz is 10.4 µs).

| Term | Path | Expected | Measured by |
|---|---|---|---|
| Stamp residual after the line fit | stamps | µs on USB; larger on pupu, where FireWire iso cycles quantise period timing at 125 µs before JACK's DLL | phase A, Q1 |
| ADC-to-stamp constant (converter group delay + whatever the reported latency misses) | stamps | tens of samples, rate dependent; RME manuals state AD latency per rate | phase B, Q3 |
| Host clock vs true second | stamps | **tens to hundreds of µs.** Both hosts run *software-timestamped* PTP: ketunkolo over a veth on a bridge, pupu unicast across a router (~0.5 ms path, asymmetry unmeasured). The ±10 µs target does not survive this term. | phase B, Q5 (composite) |
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
  grandmaster of both hosts, u-blox ZED-F9T 1PPS. ketunkolo's Babyface Pro (96 kHz) is clocked
  over ADAT from the same reference (≈0 ppm). pupu's FF400 (48 kHz) is free-running (≈ +5.6 ppm).

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
| Stamps + disciplined host clocks | locked: exact rational; free-running: tracked from stamps | delay within the measured budget of §2, stated with it |
| Neither | tracked | *offset unknown*: magnitude only, phase marked (`multi-rig.md` §4) |

If Q4 shows the USB latency moves per open (Babyface-class devices can shift by USB frames on
stream start), the practical stage 3 answer is the shared channel, and that changes what stage 3
must build.

## 5. Phases

Nothing in this study emits sound from ac2. One tool covers phases A and B.

### Tool: `ac2 selftest stamps [--keep <channel>]` (build first)
Capture-only, no daemon: opens the backend as an extra JACK client beside the running daemon
and refuses outputs (the same rule as `selftest duplex`). Per block it writes:
- sample index, frames, flags;
- `capture_ns` on the raw stamp clock, and the port's capture latency range (min and max: the
  stamp uses max, which biases it when the range is not degenerate);
- paired CLOCK_MONOTONIC and CLOCK_REALTIME reads with the bracket width.

With `--keep`: that channel's samples are written beside the CSV (f32 WAV with the block
index), for phase B's cross-correlation. Every 10 s, a side log of `chronyc tracking` and
`pmc TIME_STATUS_NP`. Analysis in Python under `tools/experiments/timing/` (like `q1/`); CSVs
and recordings from the rigs stay private.

### Phase A: stamps alone (agent-run now, both hosts, 1 h each)
The logger without `--keep`, at each rig's settings (pupu 48 k/FF400, ketunkolo 96 k/128),
then one other buffer size each.
- **Answers:** Q1 (residual on the raw stamp clock; REALTIME is slewed by chrony and would read
  as wander) and Q2 across hosts (both runs concurrent, ratio from the stamp lines mapped to
  REALTIME).
- **Also:** verifies pupu's jackd clock source.

### Phase B: one clock, one shared channel (operator cables; agent-run after)
Operator decision 2026-11-09: no PPS run to the rigs (possible, but a lot of work). The
**Antelope Orion 32HD** is the master clock: it takes the rubidium in and already clocks
ketunkolo's Babyface over ADAT. It gives pupu's FF400 word clock too.

- **B1: lock.** FF400 on word clock from the Orion. Stage 2's clock line on pupu must then read
  ≈ 0 ppm like ketunkolo's: the first check, already in the app. The ratio between the rigs is
  then exact (1 : 2 for 48 : 96 kHz). The logger repeats phase A locked (Q1 again, Q2 must be 0).
- **B2: the offset (Q7), without emitting.** Word clock fixes the ratio only; which sample of
  A goes with which sample of B is still an unknown constant per open. If the FF400 takes the
  Orion over **ADAT** instead of (or as well as) word clock, the Orion can route one source
  to both ADAT outputs. Both rigs then capture identical samples, even an unconnected input's
  own noise floor, so no sound is played. Cross-correlating the two captures gives the map
  exactly: an integer in samples, since both converters' paths are digital from the Orion on.
  That requires the Orion's rate to suit both rigs (ADAT at 96 kHz is S/MUX, half the
  channels), an operator question (§8).
- **B3: the stamp path (Q3–Q5 composite).** With B2's exact map as truth, each rig's
  stamp-derived REALTIME for the shared sample minus the other's is the stamp path's
  cross-rig error: stamps, ADC-to-stamp constants and both hosts' clock errors together.
  It's measured at both buffer sizes and again after a stream restart, a jackd restart and a
  reboot (Q4).
- **Priors:** the RME manuals' AD latency per rate. A single `jack_iodelay` round trip per rig
  checks `cap_lat`; it emits a test signal into the loopback, so the operator runs it.

What it does not give: each interface's absolute ADC latency against the true second (that
needs the PPS). Stage 3 needs only the cross-rig map, so B2 is enough for stage 3. An analog
input on a rig adds its own AD delay, a constant measurable once against the same shared
channel when that input is used.

### Phase C: network (agent-run, no audio)
A small binary against `ac2-zmq` streams block-sized messages of zeros at the real rate (48 k
mono f32 = 1.5 Mbit/s), daemon host to daemon host, over CURVE ROUTER/DEALER with credit, for
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

## 7. Write-up

The results go into `multi-device.md` §7 as measured tables, with the methods above:
- host, interface, input type, rate, buffer;
- composite mean and σ;
- Q7 map residual;
- restart spread.

`multi-rig.md` §3 is then rewritten from them, PUB corrected to the data wire as built. Stage 3
goes through the design review loop before any build.

## 8. Questions for the operator

Answered 2026-11-09: no PPS to the rigs; the Orion 32HD (rubidium in, ketunkolo's ADAT master)
gives pupu's FF400 word clock.

1. Can the FF400 also take an ADAT stream from the Orion, with one Orion source routed to both
   rigs' ADAT inputs? What rate does the Orion run at: does pupu move to 96 kHz, or does the
   Orion feed each rig at its own rate?
2. Stage 3's stream (unchanged question, restated): ac2's own sample stream over the data wire
   it already has, borrowing AES67's time model, rather than real AES67 RTP: agree?
