# Remote inputs: one rig measures with another rig's input (multi-rig stage 3)

Status: **reviewed** (Fable, 5 rounds, 2026-11-10; last verdict ready). Precondition met: `timing-study.md` phases A, B1 and C
(§7b there). Supersedes the list in `multi-rig.md` §3, which stays as the summary.

## What the operator gets

pupu plays pink noise into the PA and has its loopback. The measurement mic is on
ketunkolo, at FOH, 30 m of cable away from pupu but on the same network. Today that is two
rigs that cannot make one transfer function. With stage 3:

1. In pupu's Audio & I/O page, *Add an input from another rig…* lists the rigs the app is
   connected to and their inputs (names and roles as each rig has them). The operator picks
   *ketunkolo · in 1 · mic MM1 34804*.
2. pupu's session now has that input under a *ketunkolo* heading, with ketunkolo's row
   text (mic, calibration) and a live meter. It is a channel of pupu's session like any
   other: a new transfer function takes pupu's loopback as reference and it as measurement.
   A remote input may also be the reference (a console feed on the other rig).
3. The transfer function runs, its delay found as usual. The delay readout says it includes
   an offset between the two rigs' clocks that is not measured (§4).
4. The clocks are visible all the way: the top bar, the pane, the Connection page, the
   input row and the measurement list say whether the two rigs run on one clock, and say it
   loudly when they don't (§5).

Spectrum, RTA and SPL on a remote input need nothing of the other rig's clock. A
**transfer function between rigs** (one channel local, one remote) needs the clocks to agree,
shown by evidence (§3); one whose two channels are both on the same remote rig is a
single-clock measurement, like a local one. One whose channels are on two *different* remote
rigs is refused in this stage (*both inputs remote on different rigs: not yet*).

Not in this stage: resampling a rig that is not locked (§3.5), and a delay stated in
absolute terms between rigs (timing study B2/B3).

## 1. Model

A remote input is a **session channel**. Its number is `REMOTE_BASE + slot` (`REMOTE_BASE`
= 0x8000), not the next number after the device's channels, so that opening a device with
another channel count never renumbers a remote input under the measurements, pane picks
and recordings that name it:

```rust
pub struct SessionConfig {
    // … as today; input_channels lists remote channels too (REMOTE_BASE + slot) …
    /// Inputs of other rigs captured into this session.
    pub remote_inputs: Vec<RemoteInput>,
    /// How long audio that meets a remote input waits for it (§4); used only while
    /// `remote_inputs` is not empty.
    pub link_offset: Seconds,
}

pub struct RemoteInput {
    /// Its session channel is `REMOTE_BASE + slot` (slot < 0x8000); a slot is never
    /// reused in a session.
    pub slot: u16,
    /// The rig, by id; its name is kept for display while it is unreachable.
    pub rig: RigId,
    pub rig_name: String,
    /// Its device input channel (zero-based), as that rig numbers it.
    pub channel: u16,
}
```

Why a channel number: `TransferConfig { reference_input, measurement_input }`,
`SpectrumConfig { input }`, pairs, meters, recordings and every scene that names an input
already speak `u16` session channels. `session::validate` keeps its checks (listed once,
loopback captured; the loopback input must be local). `input_map` (device channel per block
channel), built today as a clone of `input_channels` at open, is built from the local channels
only; the fan-out learns where the remote channels come from (§2.2). A raw recording that
includes a remote channel keeps its session number and source (rig, channel) in its sidecar,
so a replay names it as recorded.

- **The stimulus and its loopback stay local.** `LoopbackRoute` is a pair on one converter;
  the timing monitor stays on the rig that plays. A transfer function's *reference input*
  may be a remote channel (operator, 2026-11-10), under the same clock rule as a remote
  measurement input.
- **Setup and calibration are the other rig's.** `InputSetup` (kind, mic, curve) and the
  calibration of a remote input are read from that rig's state at open and re-read when it
  changes; they are never copied into this rig's store. Editing them is refused with *set on
  ketunkolo* (the app offers to switch to that rig).
- **An input on this rig** is refused at add time (*that input is on this rig*). A session
  file loaded on the rig that is its remote input's source loads, with that input marked
  *that input is on this rig* and no audio, rather than refusing the load. One rig may provide
  several inputs; A taking B's input while B takes A's is allowed: each stream is one hop.
- **What B can give.** B streams only channels its own session captures. The add dialog lists
  B's captured inputs only. If B has no session, or reopens without that channel, the row says
  *ketunkolo does not capture in 1 (open it there)* or *no audio from ketunkolo (no session)*;
  the stream's incarnation ends and returns when B captures it again. A B session replaying a
  recording (`SampleClock::Synthetic`) is refused as a source (*ketunkolo is replaying a
  recording*).
- **Session file** (format bump): remote inputs with slot, rig id, name and channel. A session
  loaded while that rig is unreachable opens with the input *waiting for ketunkolo*;
  measurements on it show *no audio from ketunkolo* and resume when it answers.

## 2. The stream

### 2.1 On rig B (the source)

Rig A (the session's rig) is a client of B's daemon over B's CURVE data wire
(ROUTER/DEALER with byte credit, `docs/protocol.md` §5).

- **Topic** `audio/<channel>`: raw capture of one of B's device channels, `f32`, never
  deflated (noise does not compress; 96 kHz mono is 3.1 Mbit/s). One frame **per device
  block**: while any `audio/` topic has a subscriber, B's fan-out hands off every device
  period instead of every ~17 ms (`handoff_period`), so the stream adds one device period,
  not a hand-off, to the latency. (Phase C measured per-block frames: 1067 B every 2.67 ms.)
- **Credit.** On the data wire, a frame waiting for credit is replaced by its topic's newer
  one (`peers.rs`: credit buys the newest frame, never a backlog). That is right for curves
  and wrong for audio, so `audio/` topics are exempt: each keeps a queue up to the peer's
  window, oldest dropped and counted on B when full, and A asks for the largest window
  (`DATA_WINDOW_MAX`, 256 KiB: 680 ms of one 96 kHz channel, 230 ms of three). A marks a gap
  from any discontinuity in `start_sample`, whatever its cause. (Phase C's sender was the
  experiment's own, not the peer table.)
- **Header:** B's `start_sample`, frames, block flags (xrun, gap), B's stamp of the first
  sample's capture (`capture_ns`, B's stamp clock), B's rate, and a stream incarnation.
  B's state carries its stamp-clock → system-clock map as a line (offset, rate, epoch), fitted
  over the last minute of paired reads and refreshed every 5 s with the clock line. A stale
  map would not do: ketunkolo's stamp clock runs +8.2 ppm against its system clock, 0.5 ms a
  minute.
- **Rate.** B's session must run at A's rate; a mismatch is refused at add time (*ketunkolo
  runs 48 kHz, pupu 96 kHz*) and becomes a fault on the input if B reopens at another rate.
- **Without capture stamps.** `capture_ns` is filled by JACK, Core Audio (`input_ns`), fake
  and replay; ASIO gives none today. An ASIO source is anchored on arrival (§4) and its
  relation is *Can't tell*. A macOS rig has stamps but lands in *Can't tell* anyway: its
  system clock's status is not stated.
- **B's session restarts** (reopen, driver reset): a new incarnation. A re-anchors (§4) and
  every measurement on that input restarts with *ketunkolo's audio restarted*.

### 2.2 On rig A (the session)

- **The client** runs on a thread of its own: a sync DEALER socket (`ac2-zmq`, which already
  wraps CURVE), one per remote rig, reconnecting with backoff. It never runs on the fan-out
  thread and ac2d gains no async runtime.
- While the session has a remote input, A's fan-out hands off every device period too, so
  the joined feed's release moment (§4) is the link offset, not the next hand-off.
- Frames go to the fan-out through a bounded channel per remote rig. The fan-out keeps, per
  remote input, a ring indexed by A's sample index (§4's map).
- **The joined feed's layout.** Jobs find a channel's column with `block_index(input_map,
  ch)` over an interleaved block whose width is the map's length. The joined feed has its own
  map, the local `input_map` followed by the remote channels (`REMOTE_BASE + slot`, in
  `remote_inputs` order), handed to the jobs on it; the fan-out re-interleaves on its own
  thread (never the callback).
- **Flags for remote audio.** Today every continuity flag (`XRUN`, `DISCONTINUITY`,
  `OVERFLOW`, `CONFIG_CHANGE`) restarts both a transfer function's averages and its delay
  finder (`jobs/transfer.rs`). The joined feed adds `REMOTE_GAP` (outside
  `BREAKS_CONTINUITY`: averages restart, the finder keeps its delay, since k has not moved)
  and a slip event carrying its signed sample count (averages restart, the held delay shifts
  by exactly that count). Jobs that read no remote channel never see either.
- **Two feeds.** Jobs that read no remote channel, the session meters of local inputs and the
  timing monitor see the device blocks as today. Jobs that read a remote channel (and
  recordings that include one) get the **joined feed**: A's device blocks held back by the
  link offset, with the remote channels' samples merged in at the same sample index and their
  block statistics computed at merge. A gap or a missing remote block flags the joined block
  as a gap. Remote inputs' meters come from the joined feed.

### 2.3 Trust between rigs

Today an app pairs with a daemon. Stage 3 adds daemon-to-daemon trust, set up by the app that
holds both:

- A connects to B with **its own server key pair** as its CURVE client key (one identity per
  rig; a rig's fingerprint already means that rig).
- *Add an input from another rig…*: the app, authorized on both, asks B to admit A's public
  key **as a rig** (`auth.admit_rig { key, rig_id, name }`), and gives A B's address and the
  key the app pinned (`rig.peers` on A: rig id, address, key). Neither trusts the other on
  mDNS; the keys come from the operator's app.
- **What an admitted rig may do:** subscribe `audio/` topics, and read a filtered state:
  B's clocks (with the stamp map), session (rate, captured inputs) and those inputs' setup
  and calibration, with the events that change them. Not measurements, files, peers or
  anything else. No control commands, and no keepalives (they carry B's generator and
  health; A learns B is up from its state reads). An app is never served `audio/` nor the
  filtered state: a key admitted as an app reads *not admitted* on A's link.
- **Removing** an admitted rig (B's Connection page) drops it at once, not only at the next
  handshake: the peer table records each routing id's CURVE public key (the ZAP metadata
  property `Message::client_key` reads; `User-Id` is only the key's name), so B ends that key's subscriptions and refuses its frames. Every data message is held to the list (name and
  key, and the kind admitted), so a connection that outlived its admission is never taken
  for a new peer, and a key given another kind starts over as that kind.
- **Address.** A keeps the address the app gave; with B's key pinned, A may follow B's mDNS
  advert to a new address (a DHCP change), since the key, not the address, is the identity.
- **B's key changes** (key file set aside, new pair): A's pin fails; the input reads
  *ketunkolo's key changed: pair the rigs again from the app*. Never re-pinned silently.
- Network mode only: a daemon that listens only locally cannot serve another rig (*ketunkolo
  serves only this computer*).

## 3. Clocks between rigs

Phase A and B1 settle what a transfer function between rigs needs: an **exact ratio** (any
drift tilts the phase and smears averages, `multi-device.md` §7) and an **offset that stays
put** (the delay finder absorbs a constant one). Both hold when both rigs follow one
reference (the Orion's rubidium here; GNSS per site elsewhere), and only then.

### 3.1 The relation, per remote rig (`PeerClock`)

A computes, for each rig it takes audio from, a relation from what it has:

1. **Each rig's own clock line** (stage 2): sample clock against its disciplined system
   clock, and whether the OS says that clock is synced.
2. **The cross-rig rate**: B's sample index against B's stamps, put on system time through
   B's map; A's the same way; the ratio fitted over sliding windows. Phase A/B1: 0.34–0.40 ppm
   sd per 60 s window (stamp and host clock noise). In the pull test, 20 s windows read
   +4.1 to +6.8 ppm with the word clock out and −0.9 to +3.2 ppm locked, so a 20 s window
   alone would trip falsely.

| relation | when | cross-rig transfer function |
|---|---|---|
| **Agree** | both system clocks synced, both rigs' clock lines within ±2 ppm, cross-rig rate within ±2 ppm over the last 60 s and within ±3 ppm over the last 40 s | runs |
| **Measuring** | less than 60 s of stream since it started or re-anchored | waits (*measuring clocks…*) |
| **Not locked** | cross-rig rate beyond ±3 ppm over 40 s or beyond ±2 ppm over 60 s, or either rig's own clock line beyond ±2 ppm (*ketunkolo's interface runs +5.6 ppm against its host*: two crystals off by the same amount, or a host whose discipline wanders); back to Agree after 60 s clean | stops (§3.3) |
| **Can't tell** | either system clock unsynced or not stated (macOS, Windows), a source without capture stamps (ASIO), or the two system clocks disagreeing by more than the link offset allows (§4: *clocks of pupu and ketunkolo disagree by 3.2 ms*) | refused, said why (operator, 2026-11-10) |

±2 ppm over 60 s is 5σ of the measured noise; ±3 ppm over 40 s catches a fallback to a
crystal (the FF400's +5.6 ppm) within 40 s without tripping on the locked windows above. The
windows are checked on the rigs before the build lands (the pull is the acceptance test, §7).

**Seen through the hosts' clocks.** Each rig's rate is measured against its own disciplined
system clock, so the cross-rig rate includes how well the two hosts are disciplined. *Synced*
(`STA_UNSYNC` clear) says nothing about frequency: a host on internet NTP can wander by ppm
and slews hard after a step. The relation needs both hosts disciplined from one time source
with sub-ppm stability over a minute (PTP from a grandmaster on the reference, as on pupu and
ketunkolo; at another site, GNSS for the host as well as the interface). The *Not locked*
reason says so: *pupu and ketunkolo differ by +5.6 ppm as seen through their system clocks:
lock both interfaces to one reference, and discipline both hosts from it*.

**What *Agree* does not prove.** Stamps resolve about 2 ppm in a minute, not a lock: two
free crystals within 2 ppm of each other read *Agree*, and 2 ppm slides 0.12 ms a minute,
hundreds of degrees at 10 kHz. So the relation is named for what it shows (*clocks agree
within 2 ppm*), never *locked*, and the residual is caught by the delay track (§3.2), which
resolves it in seconds. Where a backend can read its interface's clock source and sync state
(the FF400's `primary-clock-source` and `WDClk: sync`), that is added later as evidence of a
lock; until then no rig is called locked.

### 3.2 The delay track: a cue, not a verdict

A running cross-rig transfer function's tracked delay can drift for reasons that are not
clocks: a 30 m acoustic path moves ~0.16 ms per °C, and wind moves it more
(`multi-device.md` §7 left method C's floor open for this reason). So a steady delay drift
of more than 0.05 ms a minute (≈ 0.8 ppm) over 2 minutes is shown as a cue on the pane and
the chip (*delay drifting 0.3 ms/min: path or clocks*), never a refusal. Refusals come from
stamp evidence only. Below 0.8 ppm nothing warns: over an 8 s average that is ≤ 7 µs of
slide (≤ 25° at 10 kHz). The rigs as locked measured a few ppb (timing study §7b).

### 3.3 Not locked: refuse, don't guess

A cross-rig transfer function under *Not locked* or *Can't tell* stops averaging and holds
the result it had **before the first window that failed**: the job keeps its result at each
20 s boundary while the relation reads Agree, the last three of them, because the 60 s window
can take that long to trip on a small step. A fallback to a crystal (+5.6 ppm) trips the 40 s
window after about 20 s (0.12 ms of slide), 40 s at most (0.22 ms); a 2 ppm step trips the
60 s window at up to 60 s. With no such result it shows nothing. The held
result is marked the way decision 8b marks an independent-epoch trace: the magnitude
stands, the phase is drawn faint under *clocks not locked* (or *clocks: can't tell*). Every
average taken with drifting clocks is wrong (`multi-device.md` §7), so this is a refusal.

### 3.4 After a passage through Not locked or Can't tell

The pull test showed the FF400 falling back to its crystal silently, relocking within a
second, and keeping the ~1 ms it had slid. The rate returns; the offset does not. So when the
relation returns to Agree (from Not locked, or from Can't tell; a short PTP fault such as
pupu's 16 s in phase C may not reach the kernel's sync flag at all, so it may never show),
the measurement's averages restart and its delay must follow the
step:

- **Tracking on:** the finder's tracking relocks from the held delay (`jobs/finder.rs`); the
  pane says *clocks were not locked for 3 min 10 s · delay tracked again: 13.41 ms (was
  12.38 ms)*. If tracking comes back **ambiguous**, the operator's pick is asked as today
  (decision 1c), and the phase stays faint until it is answered.
- **Tracking off:** the delay is not touched; the phase is drawn faint under *delay may have
  moved: find it again* until a find runs.

A ketunkolo-style flaky link (−228 ppm for 11 min) is just Not locked.

### 3.5 Later: rigs that are not locked

A free-running remote rig needs an asynchronous sample-rate converter on its stream, servoed
on the cross-rig rate. The stamp-derived rate moves 0.4 ppm sd per minute (86° per minute at
10 kHz), so the ratio would have to come from the audio (the delay track as a servo) or a long
average. A stage of its own, after this one runs; until then the UI says how to fix it (*lock
both interfaces to one reference*).

## 4. Buffering: a fixed link offset

As AES67 and Dante do it: a block plays in at its **capture time** plus a fixed **link
offset** (Dante's receiver latency).

- **Two anchors.** On **system time** (below) when the relation can say anything: both
  system clocks synced and both rigs stamping capture. The hosts' system clocks were measured
  to agree within −38…+7 µs over an hour (timing study §7b, phase C), so this places remote
  audio within tens of µs of the local audio it was captured with. On **arrival** otherwise
  (*Can't tell*, a source without capture stamps), or when the system-time anchor would place
  the first blocks outside [0, link offset] of their arrival (two hosts whose clocks disagree
  by more than the slack): k makes a block due `link_offset` after it arrives, as an AES67
  receiver without PTP does. Single-rig measurements on that input run; a cross-rig transfer
  function is refused (the relation is *Can't tell*, or *clocks of pupu and ketunkolo
  disagree by 3.2 ms* for the second case). The input row says which anchor it runs on.
- **Anchoring on system time.** When a stream starts (or re-anchors), A fixes `local index = remote index +
  k` from B's capture stamp of a block, put on system time through B's map, against A's own
  capture stamps on system time: k pairs samples captured at the same system time, not
  samples that arrived together. Arrival time never enters k, so transit jitter at the
  anchoring moment does not move the delay, and a re-anchor lands within the stamp error of
  the last one, up to the change in the host clocks' offset since then. k then moves only by a
  slip (below) while the stream lasts.
- **What the error is** (system-time anchor). k is off from true simultaneity by the two rigs' stamp errors
  (σ 15–17 µs, p99 30–40 µs each, §7b), the offset between the two hosts' system clocks
  (−11 µs mean, 9 µs sd per minute in phase C; not measured live), and the converters'
  latency beyond what the driver reports (JACK's `capture_ns` already subtracts the reported
  capture latency; the ADC's own filter delay is not in it). The delay finder absorbs all of
  it as a constant, which is why the delay readout says it includes an unmeasured offset
  between the rigs; a re-anchor may move it by the host-clock wander, tens of µs here.
- **The wait.** The joined feed releases A's block `link_offset` after its capture (A hands
  off per device period, §2.2), with whatever remote samples have arrived for it; a remote
  sample not there at release is **late**. B's block exists only at the end of its period
  (its stamp is the first sample's), waits up to a period for B's hand-off, crosses the
  network, and waits up to one of A's periods for A's hand-off. So the default is
  **2 × B's period + A's period + 2 ms** (phase C's one-way p99.9 was 0.9–1.3 ms): 10 ms at
  256 frames / 96 kHz on both, 66 ms at 1024 / 48 kHz. A setting on A's Audio & I/O page
  (*Wait for remote inputs: 10 ms*, default shown beside it), from that default (below it
  every remote sample is late by construction) up to 500 ms (WAN).
  Changing it re-anchors and restarts averages, so it is a session setting, not a live knob.
- **Late frames** are gaps, and a gap costs a transfer function its averages: today any
  non-contiguous block restarts the MTW and the finder (`jobs/transfer.rs`). For a gap only
  in a remote channel the map k has not moved, so the MTW restarts and the finder keeps its
  delay (a slip, below, is the case where k moves). Late frames are counted per input (*2 late frames in the last hour*) on the input row
  and the Connection page. A host stall (phase C's one 250 ms event) is one gap.
- **Too late, too often:** two late frames inside one settle span of a transfer function
  reading that input (its longest MTW stage) raise *link offset too small: raise Wait for
  remote inputs (now 10 ms)* on the input row, the top-bar chip and the pane: at that rate the
  measurement never settles, and the operator is told rather than left with a curve that never
  averages.
- **Arrival margin.** A states per input how early remote audio arrives against its
  release, the smallest over the last minute (*arrives 3.4 ms before it is needed*), on the
  Connection page; the *link offset too small* cue quotes it.
- **Slips.** Whenever the rates differ the buffer fill drifts: fast under Not locked
  (ketunkolo's −228 ppm slid 150 ms in 11 min), slowly even under Agree (1.5 ppm uses 5 ms of the
  slack in under an hour). When the ring runs dry or over, k slips by the deficit, a known
  number of samples. A slip is its own event, unlike a late frame: one gap, marked and counted
  (*ketunkolo's audio slipped 3 times*); spectrum, RTA and SPL on that input keep running; a
  transfer function reading it restarts its averages and **shifts its delay by exactly the
  slip**, said on the pane and in the measurement list (*ketunkolo's audio slipped 4.9 ms ·
  delay moved with it*). With tracking on, tracking then confirms it from there.

## 5. What the UI shows (the operator asked for clock problems to be obvious)

The rule: **wherever a cross-rig result appears, its clock relation is said next to it**, in
words; a problem is never only a colour.

- **Top bar.** While the session has a remote input, a chip per remote rig beside the rig
  drop-down: *ketunkolo · clocks agree*, *ketunkolo · measuring clocks…*, and as a warning
  chip *ketunkolo · CLOCKS NOT LOCKED +5.6 ppm*, *ketunkolo · CLOCKS: CAN'T TELL*,

  *ketunkolo · LINK OFFSET TOO SMALL*, *ketunkolo · NO AUDIO*. The chip opens the Connection
  page's *Between rigs* section. While a cross-rig transfer function is stopped, the top-bar
  banner says why and what to do: *TF 3 stopped: pupu and ketunkolo are not on one clock
  (+5.6 ppm). Lock both interfaces to one reference.* / *…can't tell whether pupu and
  ketunkolo share a clock: ketunkolo's system clock is not synced.*
- **Delay drifting** (§3.2) is a measurement's, not the rig's: it shows on that pane and as
  the measurement's cue in the list (*TF 3 · delay drifting 0.3 ms/min*), not as a rig chip.
- **The pane.** A cross-rig transfer function's title names the remote side (*TF 3 · meas
  from ketunkolo*, *· ref from ketunkolo*). Its status line: *clocks agree within 2 ppm ·
  ketunkolo*;
  stopped, the held curve's phase faint under *Clocks not locked: averaging stopped*, a verdict
  that stays with T (plot chrome hidden); after a relock, §3.4's line for a minute.
- **The delay readout** says its basis in one clause: *12.38 ms · includes the clock offset
  between pupu and ketunkolo (not measured)*. The network and the link offset are not in it
  (§4 anchors on capture time).
- **Connection page, *Between rigs*.** One block per remote rig: relation and why (*clocks
  agree: 0.0 ppm over the last minute, both system clocks synced; a lock is not proven below
  2 ppm, the delay track shows the rest*), the last change (*not
  locked 19:19:39–19:22:43; delay tracked again*), the link (*10 ms link offset · 0 late frames
  in the last hour · 3.1 Mbit/s · window 256 KiB*). On B: the rigs admitted to it, removable. Each rig's stage 2
  clock line stays above.
- **Audio & I/O.** Remote inputs under the rig's heading, with that rig's row text and a
  meter; a row whose stream is down says *no audio from ketunkolo (unreachable since 19:20)*.
- **Measurement list.** A measurement stopped by the relation shows a short cue (*clocks*),
  so it is visible with its pane closed.
- **CLI.** `ac2 status` gains *between rigs* lines with the same words.

No new keys: everything is reached through existing pages, the chip and the palette.

## 6. Wire, files, versions

- PROTO 87 (86 is the 3D spectrograph's): `SessionConfig.remote_inputs`, `link_offset`;
  `RemoteInput`; the `audio/<channel>` topic and its header; `auth.admit_rig`, `rig.peers`;
  in state, B's stamp → system map and per remote rig a `PeerClock` (relation, reason,
  cross-rig ppm and span, last change) and link counters (late frames, bytes per second).
  (`ClockRelation` is taken: the device pair's relation in `OpenSession`.)
- Session format +1 (remote inputs), rig file +1 (`rig.peers`), authorized-clients file gains
  a rig kind.
- `docs/protocol.md`, `fixtures/protocol`, `WIRE_LOCK` as usual.

## 7. Tests

- **Fake rigs:** a fake device clock at a set ppm against the wall (the fake already drifts
  output against input), and a stated `SystemClock` per fake rig instead of the runner's
  real `adjtimex` (`sysclock.rs`), so the relation never depends on CI's chrony. Two
  in-process daemons on fake devices.
- **Relation:** locked (0 ppm both); not locked (+5 ppm); a pull (+5 ppm for 3 min and back:
  Not locked within 40 s, back after a clean minute, averages restarted, delay tracked again);
  ambiguous relock; tracking off (*delay may have moved*); can't tell (one rig unsynced, or a
  source without capture stamps); a short can't-tell passage; a re-anchor returning to
  Measuring; two fake crystals 1.5 ppm apart (*Agree* plus the delay-drift cue, 0.09 ms/min);
  two fake hosts whose system clocks disagree by 10 ms (arrival anchor, refusal words); a long
  Not locked (k slips, the SPL meter on the input keeps running). All asserted
  as scene text, including the delay basis words, the top-bar banners and `ac2 status`.
- **Stream:** audio topics not replaced under a credit stall (queue, oldest dropped, counted);
  dropped frames marked, MTW restarted, finder kept; B reopens (re-anchor); B reopens at
  another rate (fault); persistently late (raise the link offset); link offset changed
  (re-anchor); buffer overrun in a long Not locked; credit window under a slow reader.
- **Session:** B without a session, without the channel, closing mid-stream, replaying;
  load with the remote rig unreachable; A reopened on a device with another
  channel count (remote channel numbers unchanged); load on the source rig itself.
- **Pairing:** admitted by the app; not admitted, refused; an admitted rig sees only the
  filtered state and no control; removal ends the stream at once; B's key changed; B's address
  changed with its key pinned.
- **Both ways:** A takes B's input while B takes A's. A remote *reference* under the
  reference-floor gate (`Routing::take`).
- **Harness:** `sim_daemon` runs one daemon; the e2e harness gains a second, each with its
  own fake device and stated system clock.
- **Rig acceptance (operator present):** pupu's loopback against ketunkolo's mic, clocks agree, a
  transfer function matching the same measurement with the mic on pupu; then the word clock
  pull: chip, banner and pane go Not locked within 40 s, and back to Agree after a clean
  minute with the delay tracked.

## 8. Stages

- **3a — stream and pairing:** `rig.peers`, `auth.admit_rig`, `audio/` topic, the client
  thread, the joined feed, remote input as session channel, meters, spectrum/RTA/SPL on it.
  A **cross-rig transfer function is refused outright** (*needs the clocks between rigs:
  next build*). Relation computed, shown on the Connection page only. The joined feed runs
  at the default link offset (§4); the setting comes in 3c.
- **3b — clocks between rigs:** the relation everywhere of §5, the refusals, §3.4's relock.
- **3c — transfer function across rigs:** remote reference or measurement, link offset
  setting, delay basis words, persistently-late cue; rig acceptance run.
- **Later:** ASRC for unlocked rigs (§3.5), a stated absolute offset (B2/B3).

## 9. Decisions

Operator, 2026-11-10:
- **Can't tell is refused.** A cross-rig transfer function needs stamp evidence that the clocks agree;
  the delay track alone never makes it run.
- **A remote reference is allowed** in the first build (3c), under the same clock rule.
