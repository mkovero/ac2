# Automatic remote inputs, levels only, and the paired rig's generator

Status: **design, not built.** Builds on `remote-inputs.md`, which stays in force wherever
this document does not replace it. Taken as given (operator, 2026-11-19): the tree shows
the rig on screen plus the rigs its daemon is paired with (mDNS name, rate, pairing state,
clock tag); the top bar carries only the shown rig's stimulus state; a click on any tree
row of any rig shows that measurement's own screen; comparing across rigs is by drag.

## 1. What the operator gets

Two rigs, paired once from the app: **pupu** (96 kHz) and **ketunkolo**.

**PA and loopback on pupu, the mic on ketunkolo.**

1. Nothing is added anywhere. pupu's tree shows ketunkolo under it with *96 kHz* and its
   clock tag. pupu's Audio & I/O page lists ketunkolo's inputs under a *ketunkolo* heading,
   each with ketunkolo's row text and a live level, marked *available*; no add, no remove.
2. New transfer function on pupu: the pickers show pupu's inputs, then *ketunkolo* with its
   inputs, a level on each row. The operator picks *pupu · loopback* and *ketunkolo · in 1 ·
   mic MM1 34804*. A row that cannot work says why in the picker, before anything starts.
3. Start. pupu's daemon subscribes that one channel of ketunkolo; the pane reads *audio from
   ketunkolo starting…*, then runs as a cross-rig transfer function does today. The
   Audio & I/O row reads *in use by TF 3*.
4. Stop or delete the measurement: the audio stream ends at once; the row is *available*
   again and keeps its live level.

**The reverse: PA and loopback on ketunkolo, the mic on pupu.**

1. The generator dialog (on pupu's screen) lists outputs by rig. The operator picks
   *ketunkolo · out 1 · PA L*. The dialog names ketunkolo in its title, shows **ketunkolo's**
   system max level and bound, and arms and starts **ketunkolo's own generator** out of
   ketunkolo's own output. No generator audio crosses the network, in either direction.
2. The transfer function on pupu takes *ketunkolo · loopback* as reference (a remote input)
   and pupu's mic as measurement. Its stimulus line: *stimulus: ketunkolo · pink −24.0 dBFS*.
3. Esc stops it as it stops a local stimulus; Shift+Esc stops every rig the app holds.
   With the mic also on ketunkolo, both channels are on one remote rig: one clock (§5).

## 2. Automatic remote inputs

### 2.1 Identity: `InputRef`

`REMOTE_BASE + slot` is a number handed out by the list this design removes; slots handed
out on demand would renumber under saved measurements. A measurement names its input by
what it is: this rig's device input, or a paired rig's as that rig numbers it.

```rust
pub enum InputRef { Local(u16), Remote { rig: RigId, channel: u16 } }
```

- **Where it goes:** the input fields of transfer, spectrum, RTA and SPL configs, a
  recording's input list, the SPL log's `input` column, and the CLI (`RIG:IN`, as
  `--remote-in` took; a bare number is local).
- **Where it does not go.** Pane contents already name the rig of each reference (`RigRef`
  + that rig's own number). `Pair`, input setup, calibration and `levels.reset` are the
  owning rig's state and keep its `u16`; on a remote row they answer *set on ketunkolo*.
  Sweeps and walk mics read local inputs only (§4.4) and keep `u16`; levels frames keep
  `u16` per rig (§3).
- **Written form:** a local input is its bare channel number, as today; a remote one is
  `{rig, channel}`. One encoding, and a rig meter naming a local input keeps its bytes, so
  `rig.json` keeps version 11 and with it the kept system max level, peers and meters. Its
  **`monitor_session` section does not survive**, see §6.
- **The raw sidecar stays at version 3**: a bump would only hide every local-only
  recording. A recording is a closed thing with its own column numbers. A column whose
  `remote` is set **is** `Remote{rig, channel}` whatever its `input` says; a new recording
  writes for it, as old ones hold, `input` = 0x8000 + its ordinal among the file's remote
  columns and `remote` with that ordinal as `slot`. The constant and the entry's type move
  from the protocol into `ac2-traces`, private to the sidecar. A bare number ≥ 0x8000 in a
  sidecar's quoted measurements names the file's column of that number, so old sidecars
  read on; `replay_inputs` and the listing key on `remote`, as built.
- **SPL log:** `# input: N` stays that rig's 1-based number; a remote input adds a `# rig:`
  line (name and id). Whether the log's format version moves depends on how strict the
  header parser is: a builder check in 1b.
- `Remote` naming the session's own rig is refused where a config is set (*that input is on
  this rig*); a session file loaded on that rig loads with the row marked so, as today.
- **Not promised:** `channel` is the source's device channel, as a local measurement names
  one; another interface there puts another socket on the number. A rig's id is a file; a
  reinstalled rig is a new rig, and references to the old one read *ketunkolo: not paired
  with this rig*, as does a session loaded on a rig never paired with the one named.

### 2.2 What is offered

**Code fact:** the daemon already links to every paired rig (`remote.peers`, set by
`rig.pair` / `rig.unpair`) from start and reads its `source.state` every second (`remote/link.rs`). This design publishes what it holds:
`remote.rigs` lists **every paired rig** with link, rate, relation and offered inputs.

**Code fact that narrows the brief:** a rig offers *the inputs its open session captures*
(`SourceSession.inputs`), not every input of its interface; with no session, none
(*ketunkolo has no session open*). §2.5 says what keeps a session open.

Rows carry that rig's row text (name, role, mic, calibration) from its `source.state`, never
a channel number (`ac2 status` shows one today as *in 32769 · mic · — · uncalibrated*).

### 2.3 Refusals, said in the picker before anything starts

A row that can never work cannot be chosen. A row whose state may change can be; the
measurement is created and **does not average** until the state allows, as a loaded
session's does today (`remote-inputs.md` §8, 3b).

| row state | words on the row | can be chosen |
|---|---|---|
| rate differs (the whole group, greyed) | *runs 48 kHz, this rig 96 kHz* | no |
| that rig replays a recording | *ketunkolo is replaying a recording* | no |
| second remote rig in one transfer function | *both inputs remote on different rigs: not yet* | no |
| sweep or walk input | *reads this rig's inputs only* (§4.4) | no |
| link down, no session, key changed, not paired | the link's words | yes; waits |
| cross-rig TF, *Measuring* | *measuring clocks…* with the seconds left | yes; waits |
| cross-rig TF, *Not locked* or *Can't tell* | the relation's words, *: will not average* | yes; refused to average |

**The evidence rule is unchanged.** As built (`control/cross_rig.rs`, `gate_of`), a cross-rig
transfer function averages only when the relation is `PeerRelation::Agree` **and** its remote
input is anchored on system time. **Two different remote rigs in one transfer function stay
refused**: their relation would be the difference of two measurements through this rig over
two independent anchors; nothing measures that pair directly.

### 2.4 Lifecycle and the joined feed

**The daemon subscribes `audio/<channel>` of a paired rig while at least one running job
(a measurement, or a recording that includes the input) reads it, and unsubscribes when
the last one stops.** Meters are not jobs: they read levels (§3). A stopped measurement
keeps its config and curves and holds no stream. Readers are counted after the whole
command, so an edit never drops a stream. Jobs on local inputs never get the joined feed.

**What a start waits for**, in order: the subscribe round trip; one link offset of held
local blocks (10 ms at 256 / 96 kHz, 66 ms at 1024 / 48 kHz); the source's stamp map (up to
10 s, `MAP_WAIT`, only just after the source opened); the anchor (up to 1 s,
`ANCHOR_WAIT`); and, for a cross-rig transfer function, a relation that already reads
*Agree* (§5.1: both sessions up for a minute). Meanwhile the pane reads *audio from
ketunkolo starting…*, then *measuring clocks…* if the minute is not yet over; an SPL meter
counts it as lost time. **After either rig reopens its session the minute is there again.**

**Bandwidth.** One 96 kHz `f32` channel is 3.1 Mbit/s; a link's window is 256 KiB for all
its channels (`DATA_WINDOW_MAX`): 680 ms of one, 230 ms of three, 85 ms of eight. A start
that would put more channels on a link than its set link offset fits is refused, with figures.

**Code fact:** today the joiner is built once per stream from `cfg.remote_inputs`
(`remote_attach`) with fixed inputs, receivers and joined map; `k` is per input, set from
one block's stamp; the cross-rig rate is fed by the rig's first listed input only; a
transfer function with both inputs on one remote rig bypasses the gate (`cross_side`);
jobs and marks find a column by its position. The joiner is therefore reshaped:

- **It lives as long as the stream**, built empty at open: state per **rig** (source
  state, incarnation, `k`) and per **column** (a ring). The first reader of any remote
  input switches the fan-out to per-period hand-off and starts holding local blocks by the
  link offset, as a session with remote inputs does at open; the last one leaving undoes it.
- **One `k` at a time per rig**, shared by all its columns (every channel of one device
  carries the same sample index). It is set once per incarnation: from the minute-smoothed
  standing offset (§5.1) if there is one; else, with a synced clock and a stamp map, on
  system time from the first block, as built; else on arrival, with its reason. It does
  not move when the smoothed offset arrives later, nor when the rig's last column goes: a
  cross-rig delay must not shift by a block's stamp error on every stop and start. A
  column added later takes `k` as it stands, which makes §5's one-clock row true.
- **Re-anchor when an arrival reason clears.** Built for a synced clock or a stamp map
  that came late (`join.rs`); **new** for `ClocksDisagree`, which today is set once. `k` is
  then set anew on system time, all the rig's columns in the same block, counted and
  marked like a slip. Since §5.1 checks age on every mark, a disagreement must have been
  clear for a minute first, and at most once per incarnation, or it would flap. Without
  the re-anchor, a rig meter started before chrony synced would leave every later
  cross-rig transfer function refused (`gate_of` needs `Anchor::SystemTime`). The
  standing relation is not reset by a re-anchor.
- **Slips** are judged only on a column that has audio and move `k` for all that rig's
  columns in the same block; a new, still empty column never moves the rig.
- **Column slots are stable.** A column is appended; a removed one stays a dead slot (no
  ring, zeros, no marks) until the joined feed has no reader, when the map is rebuilt.
- **The rate evidence leaves the audio path** (§5.1), so no input is "first".

### 2.5 Power cuts: what comes back unattended

**Code fact:** restoring the autosave opens no audio (`control/autosave.rs`). The only
unattended open is the monitor at start (`rig.monitor_start`): capture-only, only while
rig-scoped meters exist, on a config recorded only at a `session.open` made while that
holds or when the setting is switched on with audio open (`rig_meas.rs`).

So after a cut **a session measurement does not restart by itself**, local or remote; that
stays. What must come back alone is a **rig-scoped meter**; on a remote input (stage 3):

1. *On the metering rig:* a rig meter may name `InputRef::Remote`. The monitor reopens the
   device; the meter reads *waiting for ketunkolo*; the link retries (250 ms doubling to
   5 s) and follows a changed address by key, as built.
2. *On the source:* its session must be open, or it offers nothing, for ever. The monitor's
   condition becomes **rig meters exist or a rig is admitted to this one**, and the open
   config is recorded **whenever that condition becomes true with audio open**: at
   `auth.admit_rig`, when a rig meter is made, when the setting is turned on, as well as at
   `session.open`. (Open the session, then pair, is the normal order; recorded only at
   open, the first power cut would leave the source closed.)
3. **Admission thereby keeps a device open for another machine**, which may subscribe any
   captured channel unattended, and whose apps then see names, mic serials, levels,
   loudness and stimulus state of every captured input, continuously. The pairing dialog
   on the source says both; the admission's audit line and the log line of each such open
   name the rig. Removing the rig ends its subscriptions at once (as built) and, with no
   rig meter left, forgets the recorded config and closes a monitor-opened session no
   client has taken. Nothing depends on which rig boots first.

## 3. Levels only

**What exists.** Every daemon publishes `session/levels` to apps: one frame for all its
inputs at up to 30 a second, one `ballistic` and whole-frame `pair_levels` and `loudness`
in its meta; of the order of 100 kbit/s for eight inputs (off the layout; 1a measures it).

**No folding, no relay between rigs.** Who may subscribe what (`peers.rs`,
`may_subscribe`: today audio only for a rig, everything else only for an app):

| peer kind | may subscribe |
|---|---|
| admitted rig | `audio/<channel>`, `source/levels`; nothing else |
| app | everything else, `peer/<rig>/levels` included; never `audio/` nor `source/levels` |

- **`source/levels`** (new): the serving rig's **own** inputs only, the frame of
  `session/levels` with that rig's meta, plus **timing and no samples**: the newest
  block's sample index and its capture stamp **put on system time by the source** (block
  stamps are on the backend's clock, JACK's `CLOCK_MONOTONIC_RAW`, `ac2-audio`
  `stamp.rs`; the source applies its own map). No stamp or no map: no timing in the frame.
  It goes every second meter frame, 15 a second: its peaks, true peaks, clips and ballistic
  slot the highest of the two meter intervals it spans, its RMS their mean power. The clock
  fit keeps a point per 50 ms; a mark every 67 ms gives it one each, and the slope's error
  falls with the span, not the point count (about 14 kB/s for eight inputs).
- The session rig subscribes it on every link whose rig has a session; a frame waiting
  for credit is replaced by the newer one (a hole, not an error).
- **`peer/<rig>/levels`** (new): what the session rig serves its apps, that peer's frame
  as received, whole, with the peer's channel numbers, ballistic, pairs and loudness.
  `session/levels` stays local. A rig never serves rows it received.
- **Meters of a remote input always come from its own rig**, in use or not: the full
  programme meter in that rig's ballistic (named on the row when it differs), not the
  joined feed's sample peak. No frame for a second: *no level from ketunkolo*. A paired
  rig's goniometer is not relayed.

## 4. Remote outputs: the paired rig's own generator

### 4.1 What the code has today

- **A lease** (`gen.acquire` / `set` / `refresh` / `release`; `gen.stop` needs none): not
  refreshed for 1.5 s it expires with a fade, a disarm and an audit line. **Two limits:**
  the bound fixed at start (`ac2d --max-level`) and the system max level kept in
  `rig.json` (`gen.ceiling`); a raise needs `confirm_raise` and nothing armed or playing.
- **Nothing is called consent in the code.** What the operator calls consent is the lease,
  arming before firing, and the typed confirmation of a raise. This design adds no fourth.
- **An admitted rig drives nothing** (`control/request.rs`); **the app holds one client
  per rig** it is connected to.

### 4.2 Two routes, and the recommendation

**(a) The app drives the paired rig directly**, with its own client there, commands as
above. No new trust: the app's key was admitted on ketunkolo by ketunkolo's operator. The
audit there names the app. The app holds the lease; 1.5 s without it and ketunkolo fades
and disarms by itself. Cost: an app connected only to the shown rig cannot play there.

**(b) The shown rig's daemon relays** generator commands. Every app admitted on pupu could
then make sound on ketunkolo without ketunkolo admitting it, which pairing was built to
rule out; it needs a switch, a relay wire, chained leases and an audit on pupu's word.
**Recommended: (a)**: sound on another machine should need that machine's own admission of
the hand that starts it. (b) stays possible later behind a default-off switch.

- **The paired rig's limits always win**: its own daemon enforces them. The dialog shows
  the chosen rig's line (*ketunkolo: −40.0 dBFS · bound −10.0 dBFS (ac2d --max-level)*),
  edits it with that rig's `gen.ceiling`, and starts from **that rig's own generator
  state**: a level is never carried from one rig to another.
- **Moving the dialog's output to another rig while one is armed or playing is refused**
  (*stop ketunkolo's stimulus first*): two machines never change hands in one keystroke.
- **A session the monitor opened at start has no outputs** (`capture_only`; `gen.set`
  refuses *no output channels*). The dialog's group reads *ketunkolo's session has no
  outputs (opened at start): open it on ketunkolo*; reopening is a new incarnation, so the
  clock minute starts again.
- **A stimulus this app plays is always visible**, dialog closed or not: the tree heading
  of the playing rig carries the stimulus words, beside the top bar's Stop. Esc stops the
  shown rig's stimulus if it plays, else the one rig this app plays on; Shift+Esc stops
  every rig the app holds.
- **Which client.** The tree's paired rig is matched to a client the app holds by the
  **pinned server key** in its `remote.peers` entry (`rig.pair`), not by the rig id a
  daemon states in its hello. No
  match: outputs greyed under *connect to ketunkolo to play from it* (the ordinary dialog).
- **Routing.** Arm, start, level and Esc from the dialog act on the rig whose output is
  chosen. **Code fact:** a key or command without a pane goes to the shown rig
  (`state/rigs.rs`), so the dialog and the stimulus keys must carry their rig explicitly.
- **A client that does not hold the playing rig** (a kiosk on pupu only) reads *stimulus:
  ketunkolo · pink −24.0 dBFS · playing on ketunkolo: connect there to stop*. **Code fact:**
  its Shift+Esc reaches only rigs it holds; the words say so rather than imply a stop.
- **Link loss, decided.** The generator stops when **the starting app's link to the playing
  rig** is lost (lease, 1.5 s, fade). It does **not** stop when only the link *between the
  rigs* drops: the hand that started it still reaches it. The measurement on the other rig
  reads *no audio from ketunkolo*; a measurement state, not a safety state.
- **Nothing emits by itself.** After a reconnect, restart or power cut the generator is
  disarmed and the app never re-arms (*stopped: link to ketunkolo was lost*). The monitor
  at start opens capture-only, so it cannot emit at all. Ketunkolo's operator refuses by
  removing the app's key: the lease dies with the connection.

### 4.3 The reference

The reference is the playing rig's loopback, read as any input: local when the measurement
runs on the playing rig, remote when it runs on the other. `SourceInput` gains `loopback`
(that rig's `LoopbackRoute` returns on it), so the picker reads *ketunkolo · loopback* and
proposes it when a ketunkolo output is chosen. The timing monitor stays on the playing rig.
The delay readout says its basis whichever side is remote: *12.38 ms · includes the clock
offset between pupu and ketunkolo (not measured)*. A measurement stores nothing new: its
stimulus line follows the generator of the rig its reference is on; for a one-rig client
`source.state` gains that rig's stimulus in brief (armed or playing, kind, level).

### 4.4 What works across rigs, and what does not

**Works:** a dual-channel transfer function on noise or programme with output and loopback
on B and the job on A, the mic on A (under the clock rule) or on B (one clock); spectrum,
RTA and SPL of any remote input. **Does not:** a sweep or walk sweep with its output or a
mic on another rig, and periodic-noise synchronous averaging against another rig's
generator. A sweep is deconvolved against the played signal, sample for sample, on the
stream that played it; 2 ppm over a 10 s sweep slides 20 µs between its ends, 72° at
10 kHz. **Code fact:** the sweep resolves inputs against the local map only. To sweep a PA
on ketunkolo the operator clicks a ketunkolo row and sweeps there with its mics.

## 5. Clocks

Both channels on one remote rig: one clock, nothing needed between rigs, given §2.4's one
`k` per rig (*one clock: ketunkolo*). One local, one remote: `Agree` and a system-time
anchor (§2.3). Two different remote rigs: refused.

### 5.1 Standing evidence: the relation without an audio stream

Operator, 2026-11-19: "if we're receiving input levels nevertheless from remotes, could we
use that information to estimate whether we are LOCKED or ?" Yes. The relation needs
**marks** (a block's sample index with its capture time), not audio. Today they come off
the audio stream, so a cross-rig transfer function waits a minute (`SETTLE`) after the
session opens; with streams on demand that minute would come at every start.

- **One mechanism:** the timing on every `source/levels` frame (§3), on a link that is
  always up, each mark paired with this rig's own at the same system time. It is the
  quantity the joiner measures per audio block today (`join.rs`, `local_of`); windows
  (60 s, 40 s), bounds and judge do not change, only their feed. The difference of the 60 s
  fits each rig already publishes is a cross-check of the long window.
- **Age:** the relation's minute counts from when both sessions are up with the link up
  (today an audio re-anchor resets `rig.since` and both windows, `join.rs`; they move to
  the standing feed and reset on either rig's incarnation or reopened session). A hole is
  judged on the marks' own sample index and system time, not on when they arrive: a link
  that stalls and then delivers everything late has lost nothing. A real hole (over 2 s)
  is a break in the windows (the two sides fitted as segments, so a clock that moved
  meanwhile is no rate) and the minute open across it counts again; the minutes closed
  before it stand, and so does the count of marks.
- **Hold (the standing feed only):** hosts' system clocks are slewed (chrony), and a slew
  reads in a minute's fit as a rate (100 µs over a minute is about 2 ppm), on the
  cross-rig figure and on each rig's own line alike. A slew only ever makes a locked pair
  look apart, so the two verdicts are held unequally. Minutes are measured minutes: a hole
  of a few seconds drops the minute open across it, one of over a minute starts the count
  again. Each closed minute keeps two facts: its cross-rig figure, and the worst own line
  either rig stated over it (the rig and its value), so an own line beyond the rule between
  closes is not lost by being back within at the close. A rig's own line counts there only
  once it has been stated for its window, a statement period and 2 s (67 s) without going
  back to measuring or the series restarting: until then every reading is a fit that may
  still span the stream's first seconds. A minute in which such a young line read beyond
  the rule, and that is within on all else, is unclassed: no count starts from it and no
  *agree* rests on it (a clean pair with young lines still agrees at the first close). A minute is beyond when either is
  over 2 ppm, settled when both are below 1.8. *Agree* once a whole minute closes within
  the rule with no count running and the judge agreeing at the close, never beside a shown
  figure beyond the rule; between closes, after anything took it away, once the judge
  agrees three reports running. A minute closing beyond starts a count, *measuring* with
  the count while it runs, with both ways out of it (*beyond 2 ppm for 4 min running · a
  verdict at 10, agree after 2 min below 1.8*; *beyond 2 ppm in 1 of 2 min · a verdict at
  10, agree after 2 min below 1.8*). At ten
  counted: all ten cross figures beyond, one sign, is *not locked* by the cross-rig rate;
  else all ten own lines beyond, one rig and one sign, *not locked* by that rig's own line;
  else all ten beyond on one line or the other, each line keeping its sign (a free
  interface whose own line reads about the cross-rig rate, either side of the rule in
  turn), *not locked* by the newest minute's reason; else *can't tell (figure not steady:
  +0.4 … +3.1 ppm)*, the spread of the ten cross figures, never *measuring* for ever. When
  the ten cross figures are all within and only an own line came and went, the spread is
  that line's: *can't tell (ketunkolo's interface against its system clock not steady: +0.4
  … +4.0 ppm)*. Two consecutive settled minutes end a count (a margin, so a figure sitting
  at the rule does not flip in and out), and so do ten all within the rule; until then the
  count shows, so a minute beyond among ten within reads *not steady* for the minute before
  the count ends. Counts that keep starting are a clock that wanders: three started within
  30 min read *not steady* with their spread in place of *agree*, until 30 min pass with
  none started. A minute too thin to fit, or a jump of more than one in the closed minutes
  (more closed between two reports than one), breaks the run and clears the count. Anything
  the judge cannot tell (an unsynced system clock, marks late or in the future, another
  nominal rate) clears it too, and the minute open then, which spans the disturbance, is
  dropped at its close; *marks for* counts from after it, as the minutes do. Evidence that only stops (no link, no marks, a hole of a few
  seconds) keeps the count, and both *agree* and *not locked* wait for a minute closing
  after it; a hole drops the figure shown with the open minute. Holes that keep any minute
  from closing for 5 min read *can't tell (ketunkolo's timing keeps breaking off before a
  minute is in)*. The figure shown is the closed minute's own, none before the first;
  between closes it follows the live one only when that is more than 1 ppm away. A link
  respawned starts the hold again. The hold is one function of its state and each report
  (`Hold::step`), tested state by input. Ten minutes: one chrony correction would have to move the clock 1.2 ms or
  more at an update interval of 256 s or longer to read past 2 ppm that long.
- **Late marks:** marks older than the link offset in force (else the longest a link may
  run, 0.5 s) for 3 s running are *can't tell (… levels arrive N ms late: the network, or
  its system clock behind)*, and clean for 3 s before that goes.
- `ClocksDisagree` is found today on the first audio block only; the same age check runs
  on every mark. `CantTellReason::NoAudio` becomes `NoMarks`.

### 5.2 What the evidence can claim

**(1) Rate agreement.** A mark pair's error is bounded: each rig's stamp (σ 15–17 µs, p99
30–40 µs) plus the wander of the offset between the hosts' system clocks (45 µs
peak-to-peak in the one hour measured, `timing-study.md` §7b). About 0.1 ms; a rate
difference shows as that offset moving, so a window resolves about 0.1 ms / its length:
**≈ 2 ppm at 1 min** (the built bound; 0.34–0.40 ppm sd measured, `peer_clock.rs`),
**≈ 0.2 ppm at 10 min, ≈ 0.03 ppm at 1 h**.

**(2) No drift: a different, stronger test.** Follow the offset between the two sample
counts at equal system time, smoothed over a minute, since the last reset. Two interfaces
on one word clock hold it; two free crystals do not for long.

- **Claim:** the spread (max − min) of that offset since the reset, stated as the rate it
  bounds: *no drift: < 0.03 ppm over 42 min*. Not an absolute offset, and not a proof: two
  crystals 0.05 ppm apart would pass half an hour; the figure says how far the claim goes
  and tightens with time.
- **Condition:** both system clocks *Synced*, each kernel `maxerror` under a loose cap of
  a few ms (`maxerror` bounds absolute error, 0.5–3 ms on PTP hosts here, so it can only
  exclude a `timesyncd` host at hundreds of ms), and the test's own measured scatter small
  against the bound. What the test needs is the offset's *stability*; it measures that
  itself.
- **Verdict** only after the spread has stayed within N for **≥ 30 min**. N is set by the
  development check: 8 samples at 96 kHz (83 µs) over 30 min would bound 0.046 ppm, but one hour
  of data shows 45 µs of host wander, under 2× margin.
- **On exceedance:** the count resets and the tag falls back to rate agreement, with the
  figure (*drifted 0.2 ppm over 12 min*). **The drift test never says the clocks differ**:
  *not locked* comes only from the built 2 ppm rule. So a pair converging after a power
  cut (chrony settling with the kernel flag already clear) reads *agree*, then earns the
  verdict half an hour after it settles; it never reads *free*.
- **Resets:** xrun, discontinuity, incarnation, clock step or unsync on either rig; not a
  link gap.
- **Development check, done once before the drift test ships** (stage 1a, on the pupu
  rigs, nothing emitted; never a step a user runs): 24 h of the locked pair logged, and a
  word clock pull. It fixes N, the cap and the scatter bound in the code, and shows whether
  the feed trips together with the audio-fed windows. A user only ever sees the tag move by
  itself: `UNKNOWN`, `AGREE` within about a minute, `LOCKED` after about half an hour
  without drift.

**Did the earlier rule foresee this?** No. `remote-inputs.md` §3.1 withholds *locked*
because a minute of stamps resolves 2 ppm, and names what would lift it: a backend reading
the interface's sync state. The drift test answers that rule's *reason* by other means.

**The gate keeps today's rule**: `Agree` and a system-time anchor. Every tag that lets it
run is rate agreement or better; the drift verdict is never required, and never refuses.

## 6. Wire, files, versions

**Removed:** `SessionConfig.remote_inputs`, `RemoteInput`, `REMOTE_BASE`, `remote_slot`,
`RemoteInputStatus.slot`, `ac2 session open --remote-in`, the app's add dialog and remove.
**Added or changed:** `InputRef` where §2.1 lists it. `remote.rigs`: every paired rig with
link, rate, relation, offered inputs; `remote.inputs` keyed by `InputRef`, with the
measurements using each and `RemoteAudio::Idle`. `source.state`: `SourceInput.loopback`,
the stimulus in brief. Topics `source/levels` and `peer/<rig>/levels`. `PeerClock`: the
drift figures (after the development check). `CantTellReason::NoMarks`. No generator command.

**Versions.** `PROTO_VERSION` 92 → next free at merge, once per stage that touches the wire
(1a, 1b, 2); `WIRE_LOCK`, `docs/protocol.md` and the Python fixtures with each. Session
format 45 → 46 in 1b: `remote_inputs` goes with no reader for the old shape, so an older
session file or autosave is set aside whole. Raw sidecar: no bump (§2.1).

**`rig.json`: version 11 stays; one section is lost once.** It stores `monitor_session` as
the wire `SessionConfig` (`rig/file.rs`), `deny_unknown_fields`, always with
`remote_inputs`. In 1b the section gets a type of its own, capture-only by construction
(backend, input device, input channels, spare inputs, rate, period, link offset) and
itself `deny_unknown_fields`: its fields are a subset of today's, so without that old
bytes would read silently, input channels ≥ 0x8000 included. Old bytes are thus refused
(keeping the dead fields to read them would be a shim) and the section goes down the
built damage path (`rig/mod.rs`): `rig.json` is copied aside as damaged, one rig-settings
damage notice names `monitor_session`, and the salvaged file (max level, peers, meters)
is written back. Chosen over a `VERSION` bump, which would cost those too. The rule in
`rig/file.rs` is reworded: *a section's shape change costs that section; a change to
`RigMeter`, or to anything holding the max level or the peers, bumps `VERSION`.*

**Deploy of 1b**, all hosts together, per rig with a monitor at start: (1) note its
session's device, channels and rate, and copy out the rig meters' logs: their manifest
is a session file (`autosave/rig_logs.rs`), so 45 → 46 sets logs and running Leq windows
aside. (2) Deploy; expect the one damage notice; audio stays closed and the rig meters
read nothing until (3) the session is opened once (`ac2 session open …` as noted), which
records the config. (4) Restart the daemon with no client and read the audit line
*monitor at start: audio opened capture-only on <device>*; the info line *no session
recorded yet* means step 3 did not take. The system max level is not touched.

## 7. UI

Said where it arises (§1, §2.3, §3, §4.2, §10 q1); names, not numbers; no new keys.

## 8. Tests

Real daemons on the fake backend (`ac2-e2e` `sim_daemon`, two or three in process, with
stated system clocks); scene text asserted.

- **Offer:** a paired rig's captured inputs in `remote.rigs` and the picker with no command
  sent; each row of §2.3, the hard ones refused by the daemon too. **On demand:** no
  `audio/` subscription on the source while no job reads; second reader → one
  subscription; last stops → unsubscribed; stop and start → the same `k`, delay unmoved.
- **Joiner:** a second channel of one rig added mid-run → the first job's averages, `k` and
  column untouched, a shared impulse on the same sample in both (also on an arrival
  anchor); a removed column is a dead slot; an empty new column causes no slip.
- **Standing evidence:** two rigs with sessions and no stream read *Agree* after a minute;
  +5 ppm reads *not locked* after ten minutes, a slew on one host never does; a link that
  stalls and catches up keeps its minutes;
  a transfer function started then averages at once, one started 20 s after the source
  reopened waits the rest; +5 ppm, and system clocks 10 ms apart, are found with no
  stream. **Drift:** 0 ppm → the verdict after 30 min of fake time with its figure;
  0.2 ppm → reset and *agree*, never *not locked*; xrun, reopen and clock step reset it,
  a link gap does not.
- **Power cut** (`slow_`, stage 3), in the normal order: open the source's session, *then*
  admit the metering rig; rig meter on the remote input; kill both; start the metering rig
  first with its system clock unsynced, the source 20 s later, no client: the meter reads
  again, its log carries on, the source's log names the rig; when the clock syncs, one
  re-anchor, after which a cross-rig transfer function averages. Remove the rig: the
  monitor session closes. A session measurement stays stopped. Generators disarmed.
- **Files:** a `rig.json` with today's `monitor_session` → the damage notice naming it,
  the rest kept, *no session recorded yet*; after one `session.open`, the monitor reopens.
  The sidecar fixture (`raw/tests.rs`) with a remote column lists, replays, names its rig.
- **Levels:** three rigs in a chain: `source/levels` from B holds B's rows only; each row
  of §3's table refused as it says; frames stop → *no level*.
- **Generator on the paired rig**, through the app's second client matched by key: above
  that rig's level refused there; its audit names the app; the dialog's level is that
  rig's; moving to another rig while playing refused; a monitor-opened session's words;
  Esc with another rig shown. **Link-loss fade:** drop the app's connection to the playing
  rig → disarmed within the lease expiry, the fake output ramps to zero; reconnect → still
  disarmed, no `gen.set` sent. Only the rig-to-rig link dropped → still playing. A one-rig
  client: *connect there to stop*; a rig key's `gen.*` and §4.4's cases refused.
- **Rig acceptance** (operator present, sound only by his hand): both walk-throughs.

## 9. Stages

**Before any of it:** the *network as a live setting* change lands first (two socket pairs,
the network pair attached live; outgoing links keep working with the network off). It
rewrites what this design builds on in `control/remote.rs`: the tap install in
`remote_attach`, the link state in `remote_start` / `respawn_links` / `set_rig_peers`.

- **1a — standing evidence** (wire). `source/levels` between daemons with its timing; the
  standing relation computed and logged **beside** the audio-fed one; the input list and
  everything shown stay as built. Then the one-time **development check** of §5.2 on the
  pupu rigs (not a user step).
- **1b — identity and on demand** (wire; session format; the `rig.json` section of §6).
  `InputRef`; `remote_inputs` gone; the reshaped joiner on the standing relation; every
  paired rig offered; pickers and Audio & I/O rows with *available* / *in use by*;
  `peer/<rig>/levels`, since an in-use remote input must keep the meter it has today (it
  rides `session/levels` now). `RigMeter`'s input is an `InputRef` from here (bytes
  unchanged), a remote one refused until stage 3, which therefore needs no file bump.
- **2 — the paired rig's generator** (wire). `loopback` and the stimulus in brief; the
  dialog by rig, client by key, routed stimulus keys, the one-rig and no-outputs words.
- **3 — unattended** (no wire, no file bump). Rig meters on remote inputs; the widened
  monitor at start with its words, audit and un-admit. Later: re-binding references to
  another rig; route (b); a transfer function over two remote rigs.

**Builder checks, not verified here:** levels frames are computed with no app subscribed;
the levels publisher can reach block stamps; the fan-out's hand-off can be switched on a
running stream; the untagged `InputRef` reads under `deny_unknown_fields` in every config.

## 10. Questions put to the operator

All five were accepted as recommended (operator, 2026-11-19: "1-5 ok").

1. **Clock tags.** Recommended, conditional on the one-time development check of stage 1a (ours, on the
   pupu rigs; users run nothing): **`LOCKED`** = no
   drift held ≥ 30 min, the figure on hover (*no drift: < 0.03 ppm over 42 min*);
   **`AGREE`** = rates within 2 ppm, measured (the gate passes; also the first half hour
   and after any reset); **`FREE`** = the built 2 ppm rule says not locked, or another
   nominal rate; **`UNKNOWN`** = no usable evidence, measuring included, the reason on
   hover. Until that check has set the thresholds, the app ships with `AGREE` at best. This overrules
   `remote-inputs.md` §3.1 (*no rig is called locked*) on evidence that rule did not
   foresee. The alternative keeps §3.1: `AGREE` / `MEASURING` / `NOT LOCKED` / `CAN'T
   TELL`, `LOCKED` never shown.
2. **Route (a): an app must hold the paired rig to play from it, and to stop it**; a kiosk
   on one rig sees the stimulus and cannot stop it. Recommended: accept; relay later.
3. **Admission keeps the source's inputs open at start** (§2.5). Recommended: yes, said in
   the pairing dialog, audited.
4. **Pupu's apps see ketunkolo's row text, levels, loudness and stimulus in brief** without
   ketunkolo admitting them. Recommended: accept; none of it is audio or control.
5. **Pair both directions in one act** (not verified whether the app does today). Yes.

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
  to estimate whether we are LOCKED or ?" (§5; tag words: §10 q1, open.)
