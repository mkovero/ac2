# Automatic remote inputs, levels only, and the paired rig's generator

Status: **design, not built.** Builds on `remote-inputs.md` (the stream, the link offset, the
clock relation, trust between rigs), which stays in force wherever this document does not
replace it. Taken as given (operator, 2026-11-19): the measurement tree shows the rig on
screen plus the rigs its daemon is paired with, each with rate, pairing state and a clock
tag; hosts are shown by mDNS name; the top bar carries no paired-rig chips, only the shown
rig's stimulus state (*(STIM OFF)* or the playing words); a click on any tree row of any rig
shows that measurement's own screen, and comparing across rigs is by drag only.

## 1. What the operator gets

Two rigs, paired once from the app: **pupu** (96 kHz) and **ketunkolo**.

**PA and loopback on pupu, the mic on ketunkolo.**

1. Nothing is added anywhere. pupu's tree shows ketunkolo under it with *96 kHz* and its
   clock tag. pupu's Audio & I/O page lists ketunkolo's inputs under a *ketunkolo* heading,
   each with ketunkolo's row text and a live level, marked *available*.
2. New transfer function on pupu: the pickers show pupu's inputs, then *ketunkolo* with its
   inputs. The operator picks *pupu · loopback* and *ketunkolo · in 1 · mic MM1 34804*. A
   row that cannot work says why in the picker, before anything starts.
3. Start. pupu's daemon subscribes that one channel of ketunkolo; the pane reads *audio from
   ketunkolo starting…*, then runs as a cross-rig transfer function does today. The
   Audio & I/O row reads *in use by TF 3*.
4. Stop or delete the measurement: the audio stream ends at once. The row is *available*
   again and keeps its live level.

**The reverse: PA and loopback on ketunkolo, the mic on pupu.**

1. The generator dialog (on pupu's screen) lists outputs by rig. The operator picks
   *ketunkolo · out 1 · PA L*. The dialog now shows **ketunkolo's** system max level and
   bound, and arms and starts **ketunkolo's own generator** out of ketunkolo's own output.
   No generator audio crosses the network, in either direction, ever.
2. The transfer function on pupu takes *ketunkolo · loopback* as reference (a remote input)
   and pupu's mic as measurement. Its stimulus line: *stimulus: ketunkolo · pink −24.0 dBFS*.
3. Esc stops it as it stops a local stimulus; Shift+Esc stops every rig the app holds.

With the mic also on ketunkolo, both channels are on one remote rig: one clock (§5).

## 2. Automatic remote inputs

### 2.1 Identity: `InputRef`

`REMOTE_BASE + slot` is a number handed out by the list this design removes; slots handed
out on demand would renumber under saved measurements. A measurement names its input by what
it is:

```rust
pub enum InputRef {
    /// A device input of the rig that holds the measurement (zero-based).
    Local(u16),
    /// A device input of a paired rig, as that rig numbers it.
    Remote { rig: RigId, channel: u16 },
}
```

- **Where it goes:** the input fields of transfer, spectrum, RTA and SPL configs, a
  recording's input list and its sidecar columns, the SPL log's header (its `input`
  column), and the CLI (`RIG:IN`, as `--remote-in` took; a bare number is local).
- **Where it does not go.** Pane contents already name the rig of each reference
  (`RigRef` + that rig's own number). `Pair`, input setup, calibration and `levels.reset`
  are the owning rig's state and keep its `u16`; on a remote row they answer *set on
  ketunkolo*. Sweeps and walk mics (`SweepConfig`, `WalkMic.input`) read local inputs only
  (§4.5) and keep `u16`. Levels frames keep `u16` per rig (§3).
- **Written form:** a local input is its bare channel number, as today; a remote one is
  `{rig, channel}`. A file that names only local inputs keeps its bytes, so `rig.json`
  (whose rig meters hold measurement configs) needs no new version until a rig meter may
  name a remote input (stage 4). That matters: a `rig.json` set aside puts the system max
  level back at the `--max-level` bound and drops peers and the monitor at start.
- `Remote` naming the session's own rig is refused where a config is set (*that input is on
  this rig*); a session file loaded on that rig loads with the row marked so, as today.
- **Replay** keeps its rule: local columns keep their numbers, remote ones become inputs
  after the highest local one, named as recorded (`control/recording.rs`, `replay_inputs`).

**What the identity does not promise.**

- `channel` is the source's *device channel*, exactly as a local measurement names one. If
  the source opens another interface, or its channel count moves a spare input, the same
  number is another socket. `source.state` gains the capturing device's id; a remote input
  whose device differs from the one it last ran on reads *ketunkolo now captures on
  <device>: check this input* until the measurement is started again. Calibration is the
  source's, keyed by its device there, so the row always reads what that rig applies now.
- A rig's id is a file; a reinstalled rig is a new rig. Saved references then name a rig
  nobody is paired with: the row reads *ketunkolo: not paired with this rig*. The session
  keeps a table rig id → last name for such rows. `session.rebind_rig { from, to }` points
  every reference of the session at a paired rig the operator picks (*use ketunkolo
  instead*). The same words meet a session loaded on a rig never paired with the one named.

### 2.2 What is offered

**Code fact:** the daemon already keeps a link to every rig in `rig.peers` from start and
reads its `source.state` every second (`remote/link.rs`). What a paired rig captures, with
setup and calibration, is already in the session rig's daemon; this design publishes it:
`remote.rigs` lists **every paired rig** with link, rate, relation and offered inputs.

**Code fact that narrows the brief:** a rig offers *the inputs its open session captures*
(`SourceSession.inputs`), not every input of its interface. With no session it offers none
(*ketunkolo has no session open*). That stays; §2.5 says what keeps a session open.

Every picker lists this rig's inputs, then one group per paired rig under its name, each
row with that rig's row text (name, role, mic, calibration), read from it, never copied.

### 2.3 Refusals, said in the picker before anything starts

A row that can never work as configured cannot be chosen. A row whose state may change can
be chosen; the measurement is created and **does not average** until the state allows, as a
loaded session's does today (`remote-inputs.md` §8, 3b).

| row state | words on the row | can be chosen |
|---|---|---|
| rate differs | *ketunkolo runs 48 kHz, pupu 96 kHz* | no |
| that rig replays a recording | *ketunkolo is replaying a recording* | no |
| second remote rig in one transfer function | *both inputs remote on different rigs: not yet* | no |
| sweep or walk input | *reads this rig's inputs only* (§4.5) | no |
| link down, no session, key changed, not paired | the link's words | yes; waits |
| cross-rig TF, *Measuring* | *measuring clocks…* with the seconds left | yes; waits |
| cross-rig TF, *Not locked* | *clocks not locked +5.6 ppm: will not average* | yes; refused to average |
| cross-rig TF, *Can't tell* | *clocks: can't tell (reason): will not average* | yes; refused to average |

**The evidence rule is unchanged.** As built (`control/cross_rig.rs`, `gate_of`), a cross-rig
transfer function averages only when the relation is `PeerRelation::Agree` **and** its remote
input is anchored on system time. Everything else, *Can't tell* included, does not average.

**Two different remote rigs in one transfer function stay refused**: the relation between
them would be the difference of two measurements through this rig (twice the noise against
the same bound) over two independent anchors. Nothing measures that pair directly.

### 2.4 Lifecycle and the joined feed

**The daemon subscribes `audio/<channel>` of a paired rig while at least one running job
reads that input, and unsubscribes when the last one stops.** A job is a running measurement
or a recording that includes the input. Meters are not jobs: they read levels (§3).

- **Stopped but kept:** the measurement keeps its config and curves, and holds no stream.
- **Edit of a running measurement:** readers are counted after the whole command, so a job
  restarted by a config change never drops its stream.
- **Local jobs are untouched:** only readers of a remote input get the joined feed.
- **Start-up:** the first block is usable one link offset after capture (10 ms at 256 /
  96 kHz, 66 ms at 1024 / 48 kHz) plus the subscribe round trip. Until then the pane reads
  *audio from ketunkolo starting…*; an SPL meter counts it as lost time, never silence.

**Code fact:** today the joiner is built once per stream from `cfg.remote_inputs` with a
fixed input list (`control/remote.rs`, `remote_attach`); the anchor `k` is kept per input
(`join.rs`, `InputState.k`); the cross-rig rate is fed only by the rig's first listed input
(`join.rs`, `first_of_rig`); and a transfer function with both inputs on one remote rig
bypasses the clock gate (`cross_rig.rs`, `cross_side`). On demand that breaks three ways: an
idle first input would starve the rate, a column cannot come or go, and two channels of one
rig subscribed at different moments (or on an arrival anchor) can sit apart by a block
while nothing checks them. The joiner is therefore reshaped:

- **It lives as long as the stream**, built empty at open. Its state has two levels: per
  **rig** (the source state, the incarnation, the anchor) and per **column** (a ring).
- **One `k` per rig and incarnation, shared by all its columns.** Every channel of one
  device carries the same sample index, so one map serves them all. The first audio block
  from a rig sets it; a column added later takes it as it stands and anchors nothing. A
  slip moves it for all that rig's columns in the same block. Two channels of one rig are
  therefore sample-aligned with each other whatever the anchor kind, which is what makes
  §5's one-clock row true. `k` is dropped when the rig's last column goes or its
  incarnation changes; the next start anchors anew, within the stamp error of the last
  (tens of µs), and a transfer function's delay tracking takes that up.
- **Columns come and go in place**, by a command to the fan-out thread applied between
  blocks. Each joined block carries the column map it was built with; a job resolves its
  columns again only when that map changes. Adding or removing a column touches no other
  column's ring and no rig's `k`.
- **The rate evidence leaves the audio path** (§5), so no input is "first".

### 2.5 Power cuts: what comes back unattended, honestly

**Code fact:** restoring the autosave opens no audio (`control/autosave.rs`: *disarmed, no
audio I/O opened*). The only unattended open is the monitor at start: with
`rig.monitor_start` on, a restarted daemon reopens its last device capture-only, and only
while rig-scoped meters exist (`control/rig_meas.rs`).

So after a cut, **a session measurement does not restart by itself**, on a local input or a
remote one; it runs again when the operator opens the session. That is today's rule and
stays. What must come back with nobody present is a **rig-scoped meter**, and for one that
reads a remote input three things are needed:

1. *On the metering rig:* a rig meter may name `InputRef::Remote` (stage 4; the rig file
   changes there). The monitor at start reopens the device; the meter's job starts and
   reads *waiting for ketunkolo*; the link retries (250 ms doubling to 5 s) and follows a
   changed address by key, as built; the stream begins when the source captures.
2. *On the source:* its session must be open, or it offers nothing, forever. The monitor at
   start is widened: with `rig.monitor_start` on, a successful `session.open` is recorded
   **while rig meters exist or a rig is admitted to this one**, and reopened capture-only at
   start under the same condition. The source's operator decides it with the setting that
   exists; no peer can write anything on the source; capture-only cannot emit.
3. Nothing depends on which rig boots first: both sides wait in a named state.

`rig.monitor_start` off on the source means its inputs return when someone opens its
session, and the metering rig's row says *ketunkolo has no session open* until then.

### 2.6 What an old session loses

`remote_inputs` and `REMOTE_BASE` go, with no reader for the old shape. The session format
bumps; this build reads its own version only, so an older session file or autosave is set
aside whole. Measurements on remote inputs are made again by picking the inputs.

## 3. Levels only

**What exists.** Every daemon publishes `session/levels`: one frame for all inputs of its
session at up to 30 a second (`METER_FPS`), with peak, RMS, the chosen ballistic's reading,
true peak, max hold, overs and clip. Its meta has one `ballistic` and whole-frame
`pair_levels` and `loudness` (`SessionLevelsMeta`). It is served to apps, not to rigs. Size
is read off the layout (a few hundred bytes a frame for eight inputs, of the order of
100 kbit/s against 3.1 Mbit/s for one audio channel), not measured; stage 2 measures it.

**No folding, no relay between rigs.**

- **`source/levels`** is a new topic served **only to admitted rigs**, carrying **only the
  serving rig's own inputs**: the same frame as `session/levels` with that rig's meta. A
  rig never serves rows it received, so pupu reading ketunkolo gets neither a third rig's
  rows nor its own back.
- The session rig subscribes it on every link whose rig has a session, under the ordinary
  credit rule (a waiting frame is replaced by the newer one).
- **`peer/<rig>/levels`** is what the session rig serves **its apps** (never rigs): that
  peer's frame as received, whole, with the peer's own channel numbers, ballistic, pairs
  and loudness. `session/levels` goes back to local inputs only. A client talks to one
  daemon and needs no key on the other.
- **Meters of a remote input always come from its own rig**, in use or not. Today such a
  meter is the joined feed's sample peak with true peak and max hold blank; from the source
  it is the full programme meter in that rig's ballistic, named on the row when it differs
  (*ketunkolo: Type I*), and a late audio frame no longer blanks a bar.
- No frame for more than a second: *no level from ketunkolo*, never a frozen bar.
- A paired rig's goniometer is not relayed: a Programme pane on its pair shows bars and
  correlation, and says so.

## 4. Remote outputs: the paired rig's own generator

### 4.1 What the code has today

- **A lease** (`gen.acquire` / `set` / `refresh` / `release`; `gen.stop` needs none). Not
  refreshed for 1.5 s (`lease_expiry`), it expires: fade, disarm, audit *expiry*. Arming and
  firing are two steps; firing needs a session with outputs.
- **Two limits:** the bound fixed at start (`ac2d --max-level`, default −10 dBFS) and the
  system max level kept in `rig.json` (`gen.ceiling`), never above it. Any client lowers
  it; a raise needs `confirm_raise` and nothing armed or playing. `gen.set` above it is
  refused; the output path limits samples to the matching peak.
- **An audit** by client name (`ac2d::audit`: *generator Fire by laptop*).
- **Nothing is called consent in the code.** What the operator calls consent is the lease,
  arming before firing, and the typed confirmation of a raise. This design adds no fourth.
- **An admitted rig drives nothing:** `control/request.rs` refuses all but `hello` and
  `source.state` (*admitted as a rig: it may only take inputs*).
- **The app holds one client per rig** it is connected to (`state/rigs.rs`).

### 4.2 Two routes

**(a) The app drives the paired rig directly**, with its own client there, commands as above.

- Trust: none new. The app's key was admitted on ketunkolo, by ketunkolo's operator.
- Who asked: ketunkolo's audit names the app, as for any local use; no chain of identities.
- Loss: the app holds the lease on ketunkolo; 1.5 s without the app and ketunkolo fades
  and disarms by itself. The existing rule, nothing added.
- Cost: an app connected only to the shown rig cannot play from the paired rig.

**(b) The shown rig's daemon relays** generator commands over the rig-to-rig link.

- Trust: a new level. Through it **every app admitted on pupu could make sound on
  ketunkolo** without ketunkolo ever admitting that app. Pairing was built to rule that out.
- Who asked: ketunkolo logs *pupu, for laptop* on pupu's word.
- Loss: two leases in series and a rule for which loss stops what. It needs a per-rig
  switch (default off), a wire for relayed commands, and its own audit shape.
- Gain: one client suffices (a kiosk, the CLI against one rig).

### 4.3 Recommendation: route (a)

Sound on another machine should need that machine's own admission of the hand that starts
it. Route (a) gives the whole request with no new permission, no new command and the
stop-on-loss rule already in service. Route (b) stays possible later behind its switch.

- **The paired rig's limits always win**: its own daemon enforces them; the dialog can only
  ask. The dialog shows the chosen rig's line (*ketunkolo: −40.0 dBFS · bound −10.0 dBFS
  (ac2d --max-level)*) and edits it with that rig's `gen.ceiling`, a raise typed as today.
- **Which client.** The tree's paired rig is matched to a client the app holds by the
  **pinned server key** in `rig.peers`, not by the rig id a daemon states in its hello: the
  key is what the CURVE handshake proved. No match: outputs greyed under *connect to
  ketunkolo to play from it*, which opens the ordinary connect and pairing dialog.
- **Routing.** Arm, start, level and Esc from the dialog act on **the rig whose output is
  chosen**, not on the shown rig. **Code fact:** a key or command without a pane goes to
  the shown rig (`state/rigs.rs`, the message's rig), so the generator dialog and the
  stimulus keys must carry their rig explicitly, as pane messages do.
- **A client that does not hold the playing rig** (a kiosk on pupu only) reads *stimulus:
  ketunkolo · pink −24.0 dBFS · playing on ketunkolo: connect there to stop*. **Code fact:**
  its Shift+Esc reaches only rigs it holds. The words say so rather than imply a stop.
- **Refusing.** Ketunkolo's operator removes the app's key on ketunkolo's Connection page;
  the lease dies with the connection and the output fades.
- **Link loss, decided.** The generator stops when **the starting app's link to the playing
  rig** is lost (lease, 1.5 s, fade). It does **not** stop when only the link *between the
  rigs* drops: the hand that started it still reaches it and Esc works. The measurement on
  the other rig loses its reference and reads *no audio from ketunkolo*; a measurement
  state, not a safety state.
- **Nothing emits by itself.** After a reconnect, restart or power cut the generator is
  disarmed and the app never re-arms: *stopped: link to ketunkolo was lost*, until a fresh
  arm and start. The monitor at start opens capture-only, so it cannot emit at all.

### 4.4 The reference

The reference is the playing rig's loopback, read as any input: local when the measurement
runs on the playing rig, remote when it runs on the other. `SourceInput` gains `loopback`
(that rig's `LoopbackRoute` returns on it), so the picker reads *ketunkolo · loopback* and
proposes it when a ketunkolo output is chosen. The timing monitor stays on the playing rig.

A measurement stores nothing new. Its stimulus line follows the generator of **the rig its
reference input is on**; so that a one-rig client reads the truth, `source.state` gains
that rig's stimulus in brief (armed or playing, signal kind, level). The tree row of a
paired rig shows the same words. This widens what an admitted rig reads by those facts.

### 4.5 What works across rigs, and what does not

| arrangement | works? |
|---|---|
| dual-channel TF, noise or programme: output + loopback on B, mic on A, job on A | yes, under the clock rule |
| the same with the mic on B too, job on A | yes, one clock |
| output + loopback on A, mic on B, job on A | yes, as built |
| spectrum, RTA, SPL of a remote input | yes, no clock rule |
| **sweep or walk sweep on A with the output on B, or with a mic on another rig** | **no** |
| periodic-noise synchronous averaging against another rig's generator | no |

A sweep is deconvolved against the played signal, sample for sample, on the stream that
played it. Run on B with a remote mic, the clocks would have to hold far better than *agree
within 2 ppm*: 2 ppm over a 10 s sweep slides 20 µs between its ends, 72° at 10 kHz.
**Code fact:** the sweep resolves inputs against the local map only and refuses a remote
one as *not captured by the session*. To sweep a PA on ketunkolo the operator clicks a
ketunkolo row (its own screen) and sweeps there with its mics.

## 5. Clocks

| reference | measurement | needs |
|---|---|---|
| local | local | nothing |
| remote rig B | remote rig B | nothing between rigs, given §2.4's one `k` per rig; *one clock: ketunkolo* |
| local | remote B (or the reverse) | `Agree` + system-time anchor (§2.3) |
| remote B | remote C | refused |

### 5.1 Standing evidence: the relation without an audio stream (stage 1)

Operator, 2026-11-19: "if we're receiving input levels nevertheless from remotes, could we
use that information to estimate whether we are LOCKED or ?" Yes: what the relation needs
is not audio but **marks**: a block's sample index with its capture stamp. Today they are
read off the audio stream, so every start waits a minute (`SETTLE`), once per session open;
with streams on demand it would be at every start, worse than what is in the field. So the
relation moves to a standing feed in the same stage that makes streams on demand.

- **Carrier:** `source.state`, already read every second on a link that is always up. It
  gains the newest block's own mark (`start_sample`, `capture_ns`: the block's, not the
  send time), beside the stamp map it already carries. The session rig pairs each with its
  own mark at the same system time. A levels frame could carry the same mark 30 times a
  second; at windows of a minute and more that adds nothing, and the relation must not wait
  for stage 2.
- **What the joiner measures today** is exactly this: B's sample index against B's stamps
  on B's system clock, against A's the same way (`join.rs`, `local_of`), fed per audio
  block. The windows (60 s, 40 s) and bounds do not change; their feed does. The rate's
  long window is also the difference of the 60 s fits each rig already publishes
  (`capture_clock.rs`, `WINDOW_S`), which is the cross-check.
- `streaming_for` becomes *how long both sessions have run with the link up*. A link gap
  leaves a hole, not a reset: marks before and after it are on the same two sample counts.
- **Rig check before the joiner's windows go:** both feeds side by side on pupu and
  ketunkolo, the word clock pull included; they must trip together.

### 5.2 What the evidence can claim

**(1) Rate agreement.** The error of a mark pair is bounded: each rig's stamp (σ 15–17 µs,
p99 30–40 µs) plus the offset between the two hosts' system clocks (−38…+7 µs over an hour
on these rigs, `timing-study.md` §7b). Call it 0.1 ms end to end. A rate difference shows as
that offset moving, so what can be resolved is about 0.1 ms / window:

| window | resolves | note |
|---|---|---|
| 1 min | ≈ 2 ppm | the built bound; 0.34–0.40 ppm sd measured (`peer_clock.rs`) |
| 10 min | ≈ 0.2 ppm | |
| 1 h | ≈ 0.03 ppm | 30 ppb |

**(2) No drift: a different and stronger test.** Track the offset between the two sample
counts at equal system time (the same quantity as `k`), smoothed over a minute so single
stamps do not matter. Two interfaces on one word clock hold it for ever; two free crystals
cannot: 0.1 ppm apart they move 8 samples at 96 kHz in 14 minutes, and crystals wander more
than that with temperature. So:

- **Claim:** *no drift: within N samples over T* — proposed N = 8 samples at 96 kHz
  (83 µs; the mark error above, tuned on the rigs), T ≥ 15 min before the verdict. It then
  keeps running and its hover states what it has: *no drift: within 3 samples over 42 min*.
- **Needs** both hosts' system clocks held to tens of µs from one source (PTP or chrony on
  a local reference, as here). The test runs only where each host states a kernel error
  bound (`maxerror`) small against N; a host on `timesyncd` with a bound of hundreds of ms
  cannot hold up the comparison: *can't tell*. The usable `maxerror` threshold is read on
  the rigs first; it is conservative by nature and may need the bound relaxed.
- **Resets** (the offset may have stepped, so the count starts again): an xrun or
  discontinuity on either rig, either session reopening or daemon restarting (incarnation),
  a step of either system clock, either clock becoming unsynced. A link gap does not.
- **Degrades:** drift beyond N → *measured to differ* at once. A reset → back to rate
  agreement only, until T has passed again. Evidence lost → unknown.
- **What it still is not:** a reading of the interface's sync state. It cannot tell one
  word clock from two interfaces each held by its own reference, which for a measurement
  is the same thing; and it is seen through the hosts' clocks.

**Did the earlier rule foresee this?** Not this evidence. `remote-inputs.md` §3.1 withholds
*locked* because a minute of stamps resolves only 2 ppm, and names what would lift it: a
backend reading the interface's clock source and sync state. The drift test answers the
rule's *reason* (two free crystals within 2 ppm) by other means. Whether that earns the
word is the operator's call (§10 q1).

**The gate.** It keeps today's rule: *agree* (or better) and a system-time anchor. The
anchor still needs the first audio block of a stream (`gate_of`). The standing evidence
supplies the relation, so a transfer function started when the relation already stands
averages as soon as it is anchored: no minute's wait. The gate does not require *no drift*:
a rig pair up for five minutes may measure, as today.

## 6. Wire, files, versions

**Removed:** `SessionConfig.remote_inputs`, `RemoteInput`, `REMOTE_BASE`, `remote_slot`,
`RemoteInputStatus.slot`, `ac2 session open --remote-in`, the app's add dialog and remove.

**Added or changed:**
- `InputRef` where §2.1 lists it; `session.rebind_rig`; the session's rig-name table.
- `remote.rigs`: every paired rig with link, rate, relation, offered inputs; `remote.inputs`
  keyed by `InputRef`, with the measurements using each and `RemoteAudio::Idle`.
- `source.state`: the newest block's mark; capturing device id; `SourceInput.loopback`;
  the stimulus in brief. `PeerClock`: the drift test's figures (samples, span, last reset).
- Topics `source/levels` (rigs only) and `peer/<rig>/levels` (apps only).
- No generator command changes.

**Versions.** `PROTO_VERSION` 92 → next free at merge, once per stage; `WIRE_LOCK`,
`docs/protocol.md` and the Python fixtures with each. Stage 1: session format 45 → 46, raw
sidecar 3 → 4, the SPL log header's version. **Rig file 11 → 12 only in stage 4.**

**Deploys.** All hosts together. Stages 1–3 set the old autosave aside and stop listing
recordings with an older sidecar (files stay); `rig.json` is untouched. **Stage 4 sets
`rig.json` aside**, and with it the kept system max level (the daemon then starts at its
`--max-level` bound), the peers, the monitor at start and the rig meters. Its deploy, per
host: note ceiling, peers, `monitor_start` and rig meters beforehand; deploy with outputs
muted or unpatched; set the ceiling again first and read it back; re-enter peers, the
monitor at start and the meters (the standalone meter's SPL measurement included); then
cut the power once and see the meter return.

## 7. UI

Same words for the same thing everywhere; names, not numbers; a verdict only on evidence.

- **Tree.** A paired rig's row: mDNS name, rate, pairing state, clock tag (§10 q1) with its
  evidence on hover, and when used or playing, *2 inputs in use* and the stimulus words. A
  click on a row of any rig shows that measurement's own screen.
- **Top bar.** The shown rig's stimulus only. Nothing here about paired rigs; a cross-rig
  measurement that does not average says why on its pane and its tree row.
- **Audio & I/O.** This rig's inputs as today. Below, one group per paired rig with the
  tree's row text; each input with that rig's name, role, mic, calibration, a live level
  (from stage 2), and *available* or *in use by TF 3, SPL 1*. No add, no remove. Setup keys
  answer *set on ketunkolo*. A paired rig's outputs are listed read-only when the app holds
  that rig.
- **Channel pickers.** Grouped by rig, this rig first; §2.3's words; a level on each row.
- **Generator dialog.** Outputs grouped by rig; the chosen rig in the title and beside the
  level, so it is never unclear which machine will make sound. One generator per rig.
- **Pane.** As built (*TF 3 · ref from ketunkolo*, *clocks agree within 2 ppm*), plus
  *audio from ketunkolo starting…* and *one clock: ketunkolo*.
- **Connection › Between rigs** lists every paired rig, in use or not. No new keys.

## 8. Tests

Real daemons on the fake backend (`ac2-e2e` `sim_daemon`, two or three in-process daemons
with stated system clocks); scene text asserted.

- **Offer:** paired rig's captured inputs in `remote.rigs` and the picker, no command sent.
- **On demand:** no `audio/` subscription on the source while no job reads; start →
  subscribed; second reader → one subscription; last stops → unsubscribed; an edit → same
  anchor. **Joiner:** a second channel of the same rig added mid-run → the first job's
  averages and `k` untouched, both on one `k` (a shared impulse lands on the same sample in
  both, also under an arrival anchor); remove one → the other runs on.
- **Standing evidence:** two paired rigs with sessions and no stream read *agree* after a
  minute; a transfer function started then averages without *measuring clocks…*; +5 ppm on
  one fake clock trips with no stream running. **Drift test:** 0 ppm → the verdict after T
  with its figures; 0.2 ppm → *agree* by rate, then *differ* by drift, never the verdict;
  an xrun, a reopen and a system clock step each reset it; a link gap does not; a host
  with a large stated error bound → *can't tell*.
- **Identity:** other start order, save and load, replay; source on another device → the
  *check this input* words; unknown rig id → *not paired*, then `session.rebind_rig`.
- **Refusals:** each row of §2.3 as picker text; the hard ones refused by the daemon too.
- **Power cut** (`slow_`, stage 4): rig meter on a remote input; `monitor_start` on both;
  kill both; start the metering rig first, the source 20 s later, no client: the meter
  reads again and its log carries on. `monitor_start` off on the source: *has no session
  open*, no reading. A session measurement stays stopped. Generators disarmed.
- **Levels:** three rigs in a chain: `source/levels` from B holds B's rows only; a rig key
  asking `peer/…/levels` or `session/levels` is refused; an app gets B's ballistic and
  pairs on `peer/B/levels`; frames stop → *no level* within a second.
- **Generator on the paired rig**, through the app's second client matched by key: `gen.set`
  above that rig's level refused there; a raise typed; its audit names the app; Esc from
  the dialog stops the playing rig while another rig is shown. **Link-loss fade:** drop the
  app's connection to the playing rig → disarmed within the lease expiry, the fake output
  ramps to zero; reconnect → still disarmed, no `gen.set` sent. Drop only the rig-to-rig
  link → still playing. A one-rig client: the *connect there to stop* words.
- **A rig key still drives nothing.** **Sweep:** §4.5's refusals.
- **Rig acceptance** (operator present, sound only by the operator's hand): both
  walk-throughs; the word clock pull with no stream running.

## 9. Stages

1. **Identity, on demand, standing evidence** (wire; session, sidecar, SPL log header).
   `InputRef`; `remote_inputs` gone; the reshaped joiner; marks in `source.state`, the
   relation and the drift test from them; every paired rig offered; pickers and Audio & I/O
   rows with *available* / *in use by*. Idle rows show **no level yet**: §1 step 1 and §7
   are complete only with stage 2. A rig meter on a remote input is refused (*a rig meter
   reads this rig's inputs*).
2. **Levels only** (wire). `source/levels`, `peer/<rig>/levels`; remote meters from their
   own rig.
3. **The paired rig's generator** (wire only for `loopback` and the stimulus in brief; may
   ride stage 2). Dialog by rig, client by key, routed stimulus keys, the one-rig words.
4. **Unattended** (wire; **rig file**). Rig meters on remote inputs; the source's monitor
   at start for admitted rigs; the deploy of §6.

Later, each its own design: the relay of route (b); a sweep with a remote mic; a transfer
function over two remote rigs; resampling a rig that is not locked.

## 10. Open questions for the operator

1. **The tree's clock tags against the built words.** Built and decided earlier
   (`remote-inputs.md` §3.1, `ac2-scene` `cross_rig`): a minute of stamps resolves 2 ppm,
   not a lock, so the relation reads *clocks agree within 2 ppm* and no rig is called
   locked. Today's tags use `LOCKED` for that same one-minute evidence and `AGREE` for a
   weaker one (only through each rig's own system clock), so both words would mean two
   things. Options, with what a kiosk's tree row reads for ketunkolo:
   - **(a) Tags follow the relation as built:** `AGREE` / `MEASURING` / `NOT LOCKED` /
     `CAN'T TELL`. Row: *ketunkolo · 96 kHz · AGREE*; word clock out: *NOT LOCKED +5.6 ppm*.
     Nothing built changes; `LOCKED` is never shown. (The reviewer's recommendation.)
   - **(b) As (a), `LOCKED` kept for interface sync evidence** no backend gives yet.
   - **(c) Keep today's tags on today's evidence** and overrule §3.1: pane, Connection page
     and `ac2 status` change to *locked*; two free crystals within 2 ppm read `LOCKED`.
   - **(d) Tags on the standing evidence of §5.2.** `LOCKED`: no drift beyond N samples
     over ≥ T minutes, the figures on hover (*no drift: within 3 samples over 42 min*).
     `AGREE`: rates within 2 ppm, measured, but too short or too loose for the drift test
     (the first 15 minutes; after a reset). `FREE`: measured to differ, or another nominal
     rate. `UNKNOWN`: no usable evidence (no session there, unsynced or loosely bounded
     system clock, no capture stamps). Row: *ketunkolo · 96 kHz · AGREE* for a quarter of
     an hour after start, then *LOCKED*; word clock out: *FREE +5.6 ppm* within 40 s. The
     pane keeps *clocks agree within 2 ppm* where that is all there is, and gains *no
     drift…* where there is more; `NOT LOCKED` becomes `FREE` everywhere or stays, one of
     the two. This keeps the operator's four tags and gives `LOCKED` evidence two free
     crystals cannot produce. **This document's recommendation, if the operator accepts
     the drift test as earning the word**; otherwise (a).
2. **Route (a): an app must hold the paired rig to play from it, and to stop it.** A kiosk
   on one rig sees the stimulus and cannot stop it. Recommended: accept; relay later.
3. **The source reopens capture-only at start for admitted rigs** (`rig.monitor_start`,
   §2.5). Recommended: yes, under the existing setting.
4. **An admitted rig reads the other's stimulus in brief.** Recommended: yes.
5. **Pair both directions in one act** (whether the app does so today was not verified).
   Recommended: yes.
6. **Old recordings drop out of the list** with the sidecar bump. Recommended: accept.

## Decisions

Operator, 2026-11-19:
- "it would also be nice if remote inputs could be used automatically, without explicitly
  adding them in audio io settings (it could show them there though), I also expect that
  inputs stream over network only when required. also where is remote outputs?"
- The meter of a remote input nobody uses: "levels only" (the peer sends levels, not audio).
- Remote outputs: "ui should set ceiling and consent as it does for local output
  currently. reference could be remote also but both obviously initiated from rig in
  question (ie. no generator output over network)."
- "if we're receiving input levels nevertheless from remotes, could we use that information
  to estimate whether we are LOCKED or ?" (§5.1–5.2; the tag words are §10 q1, open.)
- The top bar carries no paired-rig chips; paired rigs live in the tree. A click on any
  tree row of any rig shows that measurement's own screen; comparison across rigs is by drag.
