# Automatic remote inputs, levels only, and the paired rig's generator

Status: **design, not built.** Builds on `remote-inputs.md` (the stream, the link offset, the
clock relation, trust between rigs), which stays in force wherever this document does not
replace it. It takes as given the measurement tree that shows the rig on screen plus the rigs
its daemon is paired with, each with its rate and a clock tag (`LOCKED` / `AGREE` / `FREE` /
`UNKNOWN`).

## 1. What the operator gets

Two rigs, paired once from the app: **pupu** (96 kHz) and **ketunkolo**.

**PA and loopback on pupu, the mic on ketunkolo.**

1. Nothing is added anywhere. pupu's tree shows ketunkolo under it with *96 kHz* and its
   clock tag. pupu's Audio & I/O page lists ketunkolo's inputs under a *ketunkolo* heading,
   each with ketunkolo's row text and a live level, marked *available*.
2. New transfer function on pupu: the reference picker shows pupu's inputs, then
   *ketunkolo* with its inputs. The operator picks *pupu · loopback* and *ketunkolo · in 1 ·
   mic MM1 34804*. A row that cannot work says why in the picker, before anything starts.
3. Start. pupu's daemon subscribes that one channel of ketunkolo; the pane reads *audio from
   ketunkolo starting…*, then runs as a cross-rig transfer function does today. The
   Audio & I/O row now reads *in use by TF 3*.
4. Stop or delete the measurement: the audio stream ends at once. The row is *available*
   again and keeps its live level.

**The reverse: PA and loopback on ketunkolo, the mic on pupu.**

1. The generator dialog (opened on pupu's screen) lists outputs by rig: *pupu* with its
   outputs, *ketunkolo* with its outputs. The operator picks *ketunkolo · out 1 · PA L*.
   The dialog now shows **ketunkolo's** system max level and its bound, and arms and starts
   **ketunkolo's own generator** out of ketunkolo's own output. No generator audio crosses
   the network, in either direction, ever.
2. The transfer function on pupu takes *ketunkolo · loopback* as reference (a remote input)
   and pupu's mic as measurement. Its stimulus line names where the sound comes from:
   *stimulus: ketunkolo · pink −24.0 dBFS*.
3. Esc stops it as it stops a local stimulus; Shift+Esc stops every rig.

With the mic also on ketunkolo, both channels are on one remote rig: a single-clock
measurement that needs no clock evidence between rigs (§5).

## 2. Automatic remote inputs

### 2.1 Identity: `InputRef`

`REMOTE_BASE + slot` is a number handed out by the list this design removes. Handing slots
out on demand would renumber: the same mic would be slot 0 in one session and slot 2 in the
next, under saved measurements, pane picks and recordings that name it. So an input is named
by a typed reference everywhere one is named today as a `u16` session channel:

```rust
pub enum InputRef {
    /// A device input of the rig that holds the session (zero-based).
    Local { channel: u16 },
    /// A device input of a paired rig, as that rig numbers it.
    Remote { rig: RigId, channel: u16 },
}
```

- Stable across sessions, device reopens and the order measurements start in; nothing is
  allocated, so nothing is reused or runs out.
- It replaces the `u16` in `TransferConfig` (reference, measurement), `SpectrumConfig`, SPL
  and RTA configs, recordings' input lists, `Pair`, the `Inputs` / `Pair` / `Meter` pane
  contents, the levels frames' channel lists and the raw-capture sidecar's column names.
  `SweepConfig` keeps `u16`: a sweep reads local inputs only (§4.5).
- `InputRef::Remote` naming the session's own rig is refused where a config is set (*that
  input is on this rig*); a session file loaded on the rig that is its remote input's source
  loads with the row marked so and no audio, as today.
- **Names while a rig is away.** The session file keeps a small table, rig id → last known
  name, for every rig an `InputRef` in it names, so a row reads *ketunkolo · in 1* and not an
  id while that rig is unreachable.
- Inside the daemon jobs still read columns: the joined feed maps `InputRef` → column when
  a job starts and frees a column no job reads. Columns appear nowhere on the wire.

### 2.2 What is offered

**Code fact:** the daemon already keeps a link to every rig in `rig.peers` from start, session
or not, and reads that rig's `source.state` every second (`remote/link.rs`, `SOURCE_EVERY`).
The list of what a paired rig captures, with setup and calibration, is therefore already in
the session rig's daemon; it is only not in its state, where a client could read it. This
design publishes it: `remote.rigs` lists **every paired rig**, not only those the session
takes audio from, each with its link, rate, clock relation and offered inputs.

**Code fact that narrows the brief:** a rig offers *the inputs its open session captures*
(`SourceSession.inputs`), not every input of its interface. A rig with no session offers
none, and its heading says so (*ketunkolo has no session open*). That stays: only a captured
channel has a meter, a setup row and audio to give.

Every picker (measurement kinds, recording inputs, Programme panes) lists this rig's
inputs, then one group per paired rig under its name, each row with that rig's row text
(name, role, mic, calibration), read from that rig and never copied.

### 2.3 Refusals, said in the picker before anything starts

Two classes. The first can never work as configured, so the row cannot be chosen. The second
is a state that may change, so the row can be chosen and says what would happen; the
measurement is created and held, exactly as a loaded session's is today (`remote-inputs.md`
§8, stage 3b: a session saved while the clocks agreed must still load when they don't).

| row state | words on the row | can be chosen |
|---|---|---|
| rate differs | *ketunkolo runs 48 kHz, pupu 96 kHz* | no |
| that rig replays a recording | *ketunkolo is replaying a recording* | no |
| second remote rig in one transfer function | *both inputs remote on different rigs: not yet* | no |
| sweep input | *a sweep reads this rig's inputs only* (§4.5) | no |
| link down, no session, key changed, not admitted | the link's words (*waiting for ketunkolo*, …) | yes: waits |
| cross-rig transfer function, relation *Not locked* | *clocks not locked +5.6 ppm: averaging would wait* | yes: held |
| cross-rig transfer function, relation *Can't tell* | *clocks: can't tell (…reason…): averaging would wait* | yes: held |
| cross-rig transfer function, relation *Measuring* | *measuring clocks…* with the seconds left | yes: waits |

**The evidence rule is unchanged.** As built (`control/cross_rig.rs`, `gate_of`), a cross-rig
transfer function averages only when the relation is `PeerRelation::Agree` **and** the remote
input is anchored on system time (capture stamps on both sides, both system clocks synced).
`Agree` is the cross-rig rate measured from capture stamps within ±2 ppm over 60 s and
±3 ppm over 40 s, plus each rig's own clock line within ±2 ppm. Everything else, including
agreement inferred only from the two rigs' own clock lines, holds. *Can't tell* is refused.

**Two different remote rigs in one transfer function stay refused.** pupu measures
pupu↔ketunkolo and pupu↔third; the relation ketunkolo↔third would be the difference of two
measurements (twice the noise against the same ±2 ppm bound), and the two inputs carry two
independent anchor errors. Nothing here measures that pair directly, so nothing here can
say it agrees.

### 2.4 Lifecycle: when audio streams

One rule: **the daemon subscribes `audio/<channel>` of a paired rig while at least one
running job reads that input, and unsubscribes when the last one stops.** A job is a running
measurement (transfer, spectrum, RTA, SPL), or a recording that includes the input. Meters
and Programme bars are not jobs: they read levels (§3).

- **Stopped but kept.** A measurement that is stopped keeps its config, its `InputRef`s and
  its last curves, and holds no stream. Starting it subscribes; no other step.
- **Edit of a running measurement.** The readers are counted after the whole command, so a
  job restarted by a config change never drops and re-anchors its stream.
- **Start-up.** A stream's first block is usable one link offset after it is captured
  (10 ms at 256 frames / 96 kHz on both rigs, 66 ms at 1024 / 48 kHz) plus the subscribe
  round trip; anchoring needs one stamped block. Until then the pane reads *audio from
  ketunkolo starting…* and shows no curve; an SPL meter counts that time as lost, not as
  silence. A cross-rig transfer function then reads *measuring clocks…* for up to a minute,
  as today, because the relation is measured on the audio stream's stamps. Stage 4 removes
  that minute (§5).
- **Local jobs are untouched.** Only jobs that read a remote input get the joined feed and
  its link offset; starting one never delays or restarts a local measurement.
- **Save and load.** A session file carries no list of remote inputs: its measurements name
  them, and running flags say which start. Loading subscribes for the running ones.
- **After a power cut, nobody present.** The autosave restores the session and the running
  flags; the daemon reopens the device and starts the jobs; the links to paired rigs come up
  by themselves. A running measurement on a remote input starts in *waiting for ketunkolo*
  and begins when that rig's daemon answers and its session captures the channel. No client
  is involved at any step, and nothing depends on which rig boots first.
- **A peer down at start** is that same state for as long as it lasts: the link retries
  (250 ms doubling to 5 s), the measurement waits and says so. **A peer at a new address**
  is followed by its key through its mDNS advert; **a peer that reopens** re-anchors or
  faults the input (*ketunkolo runs 48 kHz*, *does not capture in 1*); all as built.
- **The link offset** stays a session setting, in force while any remote input is read.

### 2.5 What an old session loses

`remote_inputs` and `REMOTE_BASE` go, with no reader for the old shape. The session format
bumps, and this build reads its own version only (the standing rule): an older session file
or autosave is set aside whole, not loaded without its remote inputs. Measurements on remote
inputs are made again by picking the inputs; nothing else has to be re-added, since there is
no list.

## 3. Levels only

**What exists.** Every daemon already publishes `session/levels` on its data wire: one frame
for all inputs of its session at up to 30 per second (`METER_FPS`), with sample peak, RMS,
the rig's chosen ballistic, true peak, max hold, overs and clip per input. It is served to
apps, not to admitted rigs. For eight inputs that is a few hundred bytes a frame, of the
order of 100 kbit/s for the whole rig against 3.1 Mbit/s for one channel of audio (the
size is read off the frame layout, not measured; stage 2 measures it).

**The design reuses it; no new topic.**

- An admitted rig may subscribe `session/levels` (a third thing it may do, beside `audio/`
  and `source.state`). The session rig's daemon subscribes it on every link whose rig has a
  session, at the meter rate, under the ordinary credit rule: a frame waiting for credit is
  replaced by the newer one, which is right for meters.
- The session rig folds those rows into **its own** `session/levels` frame, each named by
  `InputRef::Remote`. Its clients get every paired rig's levels as part of the one frame
  they already read; a client talks to one daemon and needs no key on the other.
- **Meters of a remote input always come from its own rig**, in use or not. Today a remote
  input's meter is the sample peak of the joined feed, with true peak and max hold blank.
  Taken from the source they are the full programme meters, in that rig's ballistic, and a
  late or lost audio frame no longer blanks a bar. The ballistic named on the row is that
  rig's (*ketunkolo: Type I*) when it differs from this rig's.
- A levels row with no frame for more than a second reads *no level from ketunkolo*, never
  a frozen bar.
- Goniometer points of a pair on another rig are not relayed; a Programme pane on such a
  pair shows bars and correlation only, and says so.

## 4. Remote outputs: the paired rig's own generator

### 4.1 What the code has today

- **The generator is leased** (`gen.acquire` / `gen.set` / `gen.refresh` / `gen.release`,
  `gen.stop` needing no lease). A lease not refreshed for 1.5 s (`lease_expiry`) expires:
  the output fades, the generator disarms, the audit says *expiry*. Arming and firing are
  two steps; firing needs a session.
- **The level limit has two parts**: the hard bound fixed when the daemon starts (`ac2d
  --max-level`, default −10 dBFS) and the system max level kept in `rig.json`
  (`gen.ceiling`), never above the bound. Any client may lower it; a raise needs
  `confirm_raise` and nothing armed or playing. `gen.set` above it is refused, and the
  output path limits samples to the matching peak.
- **Every action is audited** by client name (`ac2d::audit`: *generator Fire by laptop*)
  and kept as `generator.last_action`.
- **There is nothing called consent in the code.** What the operator calls consent is the
  sum of: holding the lease, arming before firing, and the typed confirmation of a raise.
  This design keeps exactly those and adds no fourth thing.
- **An admitted rig may not drive anything.** `control/request.rs` refuses every command
  from a rig but `hello` and `source.state` (*admitted as a rig: it may only take inputs*).
- **The app holds one client per rig** it is connected to (`state/rigs.rs`), shows another
  rig's stimulus as a top-bar chip (*▶ ketunkolo −24 dBFS*) and stops every rig on Shift+Esc.

### 4.2 Two routes

**(a) The app drives the paired rig directly**, with its own client there, using the
commands above unchanged.

- Trust: none new. The app's key is admitted on ketunkolo as an app, by ketunkolo's
  operator, which is what being an operator of that rig means today.
- Who asked: ketunkolo's audit names the app (*generator Fire by laptop*), as for any
  local use. There is no chain of identities to forge or mis-state.
- Loss: the lease is held by the app on ketunkolo. If the app loses ketunkolo for 1.5 s,
  ketunkolo fades out and disarms by itself: the existing rule, nothing added.
- Cost: an app connected only to the shown rig cannot drive the paired rig until it
  connects there. The tree requires only the *daemons* to be paired.

**(b) The shown rig's daemon relays** generator commands to the paired rig over the
rig-to-rig link.

- Trust: a new level. An admitted rig would be allowed to make sound on another machine,
  and through it **every app admitted on pupu would become able to make sound on
  ketunkolo** without ketunkolo's operator ever admitting that app. Pairing was built to
  rule this out.
- Who asked: ketunkolo must log *pupu, for laptop* on pupu's word.
- Loss: two leases in series (app → pupu, pupu → ketunkolo) and a rule for which loss
  stops what. It also needs a per-rig switch (*other rigs may drive this rig's generator*,
  default off), a wire for relayed commands, and its own audit shape.
- Gain: one client suffices (a kiosk, the CLI against one rig).

### 4.3 Recommendation: route (a)

Sound on another machine should need that machine's own admission of the hand that starts
it. Route (a) gives the whole request (ceiling, arm, start and stop from the shown rig's
screen, as for a local output) with no new permission, no new command and the stop-on-loss
rule already in service. Route (b) stays possible later behind its default-off switch.

Under (a):

- **The paired rig's limits always win**, because its own daemon enforces them: `gen.set`
  above its system max level is refused there, and its bound is its start flag. The dialog
  can only ask.
- **Ceiling.** The dialog shows the chosen rig's line (*ketunkolo: −40.0 dBFS · bound
  −10.0 dBFS (ac2d --max-level)*) and edits it with that rig's `gen.ceiling`, the raise
  confirmed by typing as today.
- **Not connected there.** A paired rig the app holds no client for shows its outputs
  greyed under *connect to ketunkolo to play from it*; choosing it opens the ordinary
  connect and pairing dialog for that rig. It is never done silently.
- **Refusing.** Ketunkolo's operator removes the app's key on ketunkolo's Connection page:
  the lease dies with the connection and the output fades. No extra switch is needed,
  because no extra permission was given.
- **Link loss, decided.** A generator started from an app stops when **that app's link to
  the playing rig** is lost (lease expiry, 1.5 s, fade). It does **not** stop when the link
  *between the two rigs* drops while the app still reaches the playing rig: the hand that
  started it can still stop it, Esc works, the chip shows it playing. The measurement on
  the other rig loses its remote reference and holds (*no audio from ketunkolo*), which is
  a measurement state, not a safety state.
- **Nothing emits by itself.** After a reconnect, a daemon restart or a power cut the
  generator is disarmed (a restored session opens *disarmed*, as built) and the app never
  re-arms or re-fires on its own: the dialog reads *stopped: link to ketunkolo was lost*
  and waits for a fresh arm and start.
- **Fade** on stop, on expiry and on a ceiling lowered under the playing level is the
  playing rig's own, as built.

### 4.4 The reference

The reference for a transfer function is the playing rig's loopback, read as any input: a
local input when the measurement runs on the playing rig, a remote input when it runs on
the other. For this `SourceInput` gains `loopback: bool` (that rig's `LoopbackRoute` returns
on it), so the picker reads *ketunkolo · loopback* and proposes it as reference when a
ketunkolo output is chosen. The timing monitor (generator against loopback) stays on the
playing rig, where both ends are on one converter.

A measurement stores nothing about the stimulus that it does not store today. Its stimulus
line follows the generator of **the rig its reference input is on**. So that a client with
no connection to that rig still reads the truth, `source.state` gains that rig's stimulus
in brief (armed or playing, signal kind, level): *stimulus: ketunkolo · pink −24.0 dBFS*,
or *no reference: ketunkolo's stimulus is not playing*. This widens what an admitted rig
may read by those three facts (§10).

### 4.5 What works across rigs, and what does not

| arrangement | works? |
|---|---|
| dual-channel transfer function, noise or programme: output + loopback on B, mic on A, job on A | yes, under the clock rule (§5) |
| the same with the mic on B too, job on A | yes, single clock, no clock rule |
| output + loopback on A, mic on B, job on A | yes, as built |
| spectrum, RTA, SPL of a remote input while another rig plays | yes, no clock rule |
| **sweep run on A with the output on B** | **no** |
| **sweep run on B with a mic on A** | **no, not in this design** |
| periodic-noise synchronous averaging against a generator on another rig | no |

A sweep is deconvolved against the signal that was played, sample for sample, on the stream
that played it. The job on A has no sample-exact knowledge of what B played or when. Run on
B with a remote mic, the two clocks would have to hold through the sweep to far better than
*agree within 2 ppm*: 2 ppm over a 10 s sweep slides 20 µs between its low and high end,
72° at 10 kHz, and stamps cannot show less. So a sweep stays on one rig, inputs and outputs
(**code fact:** the sweep already resolves its inputs against the local map only and
refuses a remote one as *not captured by the session*; the picker now says why). To sweep a
PA on ketunkolo, the operator shows ketunkolo and sweeps there, with ketunkolo's mics.

## 5. Clocks

| reference | measurement | clocks involved | needs |
|---|---|---|---|
| local | local | one | nothing |
| remote rig B | remote rig B | one (B's) | nothing between rigs; shown as *single clock: ketunkolo* |
| local | remote B (or the reverse) | two | measured agreement + system-time anchor (§2.3) |
| remote B | remote C | three | refused |

Spectrum, RTA, SPL and levels of any input need nothing of another rig's clock.

**The tree's tags against the gate.** The gate accepts one thing only: agreement *measured
between the rigs* from capture stamps. In the tree's vocabulary that is `LOCKED`. `AGREE`
(indirect: each rig's own clock line agrees with its host, nothing measured across),
`FREE` and `UNKNOWN` do not let a cross-rig transfer function average. **A contradiction to
settle (§10):** `remote-inputs.md` §3.1 rules that no rig is ever called *locked*, because
stamps resolve 2 ppm and two free crystals within 2 ppm read the same; the built words are
*clocks agree within 2 ppm*. The tree's `LOCKED` names that same evidence with the word the
earlier decision withheld.

**Measured without audio (stage 4).** Today the cross-rig rate is fitted on the audio
stream, so a paired rig nobody takes audio from has no measured relation, and every
cross-rig transfer function waits a minute after start. With streams on demand that minute
would come at every start. The same evidence (a block's sample index against its capture
stamp) can ride each `session/levels` frame, 30 a second, on a link that is always up: the
relation is then measured for every paired rig with a session, the tree's tag is the
measured one, the picker's clock words are current, and a transfer function averages as
soon as its stream is anchored. Whether 30 stamp pairs a second hold the 0.34–0.40 ppm sd
per minute measured on per-block stamps is checked on pupu and ketunkolo before the gate
relies on it; the thresholds do not move.

## 6. Wire, files, versions

**Removed:** `SessionConfig.remote_inputs`; `RemoteInput`, `REMOTE_BASE`, `remote_slot`;
`RemoteInputStatus.slot`; `ac2 session open --remote-in`; the app's *Add an input from
another rig…* dialog and the row's remove.

**Added or changed:**
- `InputRef` in every config, pane content, pair, recording request and levels channel
  list that names an input (§2.1).
- `remote.rigs`: every paired rig, with link, rate, relation and offered inputs (setup,
  calibration, `loopback`); `remote.inputs`: one entry per remote input a config names,
  keyed by `InputRef`, with the measurements that use it and `RemoteAudio` gaining
  `Idle` (offered, no reader).
- `session/levels`: rows for remote inputs; the topic served to admitted rigs; in stage 4
  each frame carries its block's sample index, capture stamp and incarnation.
- `source.state`: `SourceInput.loopback`; the stimulus in brief (§4.4).
- No new command. No generator command changes.

**Versions.** `PROTO_VERSION` 92 → next free at merge, once per stage that touches the wire
(stages 1, 2, 4; stage 3 only if it lands apart from the `source.state` fields).
`fixtures/protocol/WIRE_LOCK`, `docs/protocol.md` and the Python fixtures with each. Session
format 45 → 46. Rig file 11 → 12 (rig-scoped meters name their input by `InputRef`). Raw
capture sidecar 3 → 4 (columns named by `InputRef`). All three file bumps land in stage 1,
so hosts pay once.

**What a deploy needs.** All hosts together (no wire compatibility). The old autosave and
`rig.json` are set aside as `docs/rigs/pupu.md` describes. **A `rig.json` set aside takes
the kept system max level with it: the daemon starts at its `--max-level` bound** until the
level is set again, and paired rigs and rig-scoped meters are entered again. The deploy
re-applies ceiling, peers and rig meters on each host before anyone plays, and re-creates
the standalone meter's SPL measurement. Recordings with an older sidecar stop being listed
(their WAV files stay).

## 7. UI

Same words for the same thing everywhere; names, not numbers; a verdict only on evidence.

- **Audio & I/O.** This rig's inputs as today. Below, one group per paired rig under its
  name with rate and clock tag (the tree's row text). Each row: that rig's name, role, mic,
  calibration, a live level, and *available* or *in use by TF 3, SPL 1*. No add, no remove.
  Setup keys there answer *set on ketunkolo* and offer to show that rig. A paired rig's
  outputs are listed read-only under its name when the app is connected there.
- **Channel pickers.** Grouped by rig, this rig first; §2.3's words on rows; a remote row
  shows its level, so a dead mic is seen before it is picked.
- **Generator dialog.** Outputs grouped by rig. The chosen rig is named in the title and
  beside the level (*ketunkolo · system max level −40.0 dBFS · bound −10.0 dBFS*): it is
  never unclear which machine will make sound. The arm line names the rig, as built. One
  generator per rig; two rigs playing are two chips.
- **Tree.** A paired rig's row gains *2 inputs in use* and, while it plays, the chip words.
- **Pane.** As built (*TF 3 · ref from ketunkolo*, *clocks agree within 2 ppm*), plus
  *audio from ketunkolo starting…* and *single clock: ketunkolo*.
- **Connection › Between rigs** lists every paired rig, in use or not. No new keys.

## 8. Tests

Real daemons on the fake backend (`ac2-e2e` `sim_daemon`, two in-process daemons with
stated system clocks); scene text asserted.

- **Offer:** pair two daemons: the paired rig's captured inputs are in `remote.rigs` and
  the picker's scene with their row text, no command sent; no session there says so.
- **On demand:** no `audio/` subscription on the source while no job reads (its peer
  table; bytes per second); start → subscribed, curve arrives; second reader → one
  subscription; last reader stops → unsubscribed; an edit → anchor unchanged.
- **Identity:** reopen either device with another channel count, start in another order,
  save and load: every measurement reads the input it named.
- **Refusals:** each row of §2.3 as picker text; the hard ones refused by the daemon too.
- **Power cut** (`slow_`): kill both daemons mid-measurement; start the session rig first,
  the source 20 s later, no client connected: the measurement runs again (read by a client
  attached afterwards), generators on both disarmed. Also: peer at a new address.
- **Levels only:** remote rows in the session rig's levels frame with no audio stream,
  with full programme meters; frames stop → *no level* within a second.
- **Generator on the paired rig**, through the app's second client: `gen.set` above that
  rig's system max level refused there; a raise needs the typed confirmation; that rig's
  audit names the app. **Link-loss fade:** drop the app's connection to the playing rig →
  disarmed within the lease expiry, the fake backend's output ramps to zero; reconnect →
  still disarmed, no `gen.set` sent. Drop only the rig-to-rig link → still playing, the
  measurement holds with its words. No key on the paired rig → greyed, nothing sent.
- **A rig key still drives nothing:** every `gen.*` from it refused. **Sweep:** a remote
  input or another rig's output refused with §4.5's words.
- **Rig acceptance** (operator present, sound only by the operator's hand): both
  walk-throughs on pupu and ketunkolo.

## 9. Stages

Each is deployable on its own.

1. **Identity and lifecycle** (wire bump; session, rig file and sidecar bumps).
   `InputRef`; `remote_inputs` gone; every paired rig's inputs offered; streams on demand;
   pickers and the Audio & I/O rows with *available* / *in use by*; idle rows show no level
   yet.
2. **Levels only** (wire bump). `session/levels` to admitted rigs, folded into the session
   rig's frame; live levels on every offered row; remote meters from their own rig.
3. **The paired rig's generator** (wire bump only for the two `source.state` additions;
   they can ride stage 2). Generator dialog by rig, ceiling line per rig, reference
   proposal, stimulus line following the reference's rig, greyed outputs with the connect
   offer.
4. **Relation without audio** (wire bump). Stamps on levels frames; measured relation for
   every paired rig; no minute's wait at start. After the rig check in §5.

Later, each its own design: the relay of route (b); a sweep with a remote mic; a transfer
function over two remote rigs; resampling a rig that is not locked.

## 10. Open questions for the operator

1. **Route (a) means an app must be connected to the paired rig to play from it** (a kiosk
   tied to one rig cannot). Recommended: accept; the relay is a later, default-off option.
2. **The tree's `LOCKED` against the earlier rule that no rig is called locked.**
   Recommended: the tree's tag for measured agreement keeps the tag's short form but its
   tip and every sentence keep *clocks agree within 2 ppm*; or rename the tag. One of the
   two words must go, so that the same evidence reads the same everywhere.
3. **Until stage 4, every start of a cross-rig transfer function waits a minute.**
   Recommended: accept for stages 1–3 rather than keep idle streams warm (which would break
   *stream only when required*); do stage 4 soon after.
4. **An admitted rig reads the other's stimulus in brief** (armed or playing, kind,
   level). Recommended: yes; it is what the top-bar chip already tells any connected app,
   and without it a one-rig client cannot say why a remote reference is silent.
5. **Pairing in one act, both ways.** The reverse walk-through needs ketunkolo paired on
   pupu; pupu's mic into a ketunkolo session needs the other direction. Recommended: the
   app pairs both directions whenever it pairs two rigs.
6. **Old recordings drop out of the list** with the sidecar bump. Recommended: accept, as
   with the earlier sidecar bumps; the files stay on disk.

## Decisions

Operator, 2026-11-19:
- "it would also be nice if remote inputs could be used automatically, without explicitly
  adding them in audio io settings (it could show them there though), I also expect that
  inputs stream over network only when required. also where is remote outputs?"
- The meter of a remote input nobody uses: "levels only" (the peer sends levels, not
  audio).
- Remote outputs: "ui should set ceiling and consent as it does for local output
  currently. reference could be remote also but both obviously initiated from rig in
  question (ie. no generator output over network)."
