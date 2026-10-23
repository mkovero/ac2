# Many rigs in one app: tabs, mixed panes, remote inputs, clocks across hosts

One app connects to any number of daemons. Each is a full rig — session, measurement tree,
stimulus, calibrations — exactly as the single daemon is today; nothing is a watered-down
"viewer" mode. Panes may show any rig's measurement side by side, and a measurement on one
rig may take an input from another. Decisions N1–N6 in `open-questions.md`.

Operator's framing (2026-10-23): keep every connection functionally identical to the first;
switch between them; by default they are sub-sessions with tabs; nothing prevents including
another daemon's channels in a measurement. Different magic states would confuse, and there
is no reason to cripple the other rigs. The math is the daemon's; the app renders streams.
Both rigs in use (pupu, ketunkolo) take their system clock from PTP, so timing between
daemons can be sharp.

## What the operator gets

- **Connect…** adds a rig: browse (`_ac2._tcp`, network mode) or a typed address, paired per
  rig as today. The embedded daemon is a rig like any other (*this computer*).
- **Tabs**, one per rig, named by the rig (never trusted beyond display). The active tab is
  the rig the single-rig keys talk to: Space arms *its* stimulus, Enter opens *its* session,
  the tree is *its* tree, Settings' Audio & I/O is *its* devices. Switching tabs switches all
  of that; nothing else changes.
- **Panes are the app's**, not a tab's: the pane tree stays when the tab changes, and a pane
  shows a measurement of any connected rig. A pane's title names the rig when it is not the
  active one (*pupu · Stage*). Pick and drag work across rigs: the picker lists every rig's
  measurements under a heading per rig.
- **Remote inputs**: a rig's session may include an input channel of another rig (*ketunkolo
  · in 3*). It appears on the Audio & I/O page under its rig, with its name, domain and
  calibration as that rig states them, and goes into measurements like a local input.
- **Clocks**: the Connection page says, per rig, how its sample clock runs against its system
  clock and whether the system clock is disciplined; between two rigs, the offset and drift of
  their capture streams. A cross-rig measurement states which clock relation it rests on.

The two rigs in use: a Programme pane with ketunkolo's console-feed loudness beside pupu's
SPL meter at the place, in one layout, on one screen (or one Pi).

## 1. One state per rig, one layout per app (N1)

Today's `State` is the state of one rig: connection, session, tree, selection, stimulus,
dialogs that belong to a session. It stays as it is and becomes `RigState`; the app holds

```rust
pub struct App {
    rigs: Vec<RigState>,          // in tab order; at least the embedded or first rig
    active: RigId,                // the tab shown
    layout: PaneTree,             // panes with cross-rig sources (§2)
    settings: Settings,           // app-wide (how the operator likes to read)
}
```

`RigId` is the server's CURVE public key: stable across reconnects, address changes and
renames, and it is what pairing already keys on. Display name from the hello (and
discovery), re-read on each connect.

Every reducer event names its rig; the link thread tags what it delivers with its `RigId`.
Keys are routed by **one rule**: a key scoped to a pane goes to the pane's rig (R on a
Programme pane of ketunkolo resets ketunkolo's hold; T hides that pane's chrome); every
other key goes to the active tab's rig. No key ever acts on a rig the operator is neither
looking at (tab) nor pointing at (focused pane).

A rig that drops out keeps its tab and its panes: panes grey with *pupu unreachable ·
retrying* and the connection retries as today. Closing a tab closes that connection (its
stimulus stopped and lease released first, `QUIT_GRACE`); its panes stay in the layout
saying *pupu not connected* until re-connected or re-pointed. Quit stops every rig's held
stimulus in parallel, each within `QUIT_GRACE`.

### Stimulus awareness

Stimulus runs per rig under its own lease. The top bar shows the active rig's stimulus as
today, and a chip per *other* rig whose stimulus this app holds (*▶ pupu −20 dBFS*): sound
coming out of a system you are not looking at is never silent in the UI. Escape on the
active rig stops its stimulus; Shift+Escape stops every rig's.

## 2. Pane sources (N2)

```rust
pub struct PaneSource { pub rig: RigRef, pub meas: MeasId }
pub enum RigRef { Active, Rig { key: ServerKey, name: String } }
```

`Active` is the default and what a single-rig app has always meant: the pane follows the
active tab. A pane pointed at a specific rig keeps it across tab switches. The picker and a
drop set `Rig` when the source is not the active rig.

Layout prefs save `Rig` by key and name. A session file is one rig's (a session is a daemon
concept; nothing changes there), and its panes may reference other rigs by key and name:
loading it on an app without that rig shows those panes as *ketunkolo not connected* with
the measurement's name, so the layout survives and says what is missing.

## 3. Remote inputs (N3)

A rig's session includes an input channel of another rig by subscribing to it: daemon A
asks daemon B for channel `c` and B publishes its capture of `c` to A.

- **Wire**: `audio.subscribe { channels }` on B opens a per-subscriber CURVE PUB stream of
  f32 blocks, sample-indexed in B's capture clock, each block stamped with B's wall time at
  its first sample (the stamp the fan-out already puts on every block). `audio.unsubscribe`
  and the lease-like expiry of a silent subscriber end it. 48 kHz mono f32 is 1.5 Mbit/s a
  channel; the Connection page states the rate. `PROTO_VERSION` bump.
- **Session**: `SessionConfig.input_channels` grows a remote form, `RemoteInput { rig,
  channel }`, beside device channels. `InputSetup` of a remote input is **B's**: name,
  domain, mic, curve and calibration are read from B at open (and re-read when B says they
  changed), never copied into A's store — the mic is on B's input and B is where it was
  calibrated. A's Audio & I/O page shows the remote input under a *ketunkolo* heading with
  B's row text; editing it says *set on ketunkolo*.
- **Where a measurement runs**: on the rig whose outputs carry its stimulus (the reference is
  that rig's loopback). Cross-rig transfer: stimulus out of B, B's loopback reference, A's
  mic as a remote input into B's session — or the other way round. Programme material
  (no stimulus) runs wherever the operator makes the measurement. The app's picker refuses
  a measurement whose reference and stimulus are on different rigs by name, since the
  loopback would not be a loopback.
- **Timing**: a remote input is a second capture clock, the case `multi-device.md` §5 solves
  for two devices on one host, with one difference: the common reference is not a loopback
  but the two hosts' wall clocks, read at every block (§4). The drift model and its
  statement to the operator (*drift 2.1 ppm · resampled* or refused) are reused as they
  stand; the offset comes from the wall stamps instead of a correlation.

Why not netjack: it was tried (2026-10-11) and failed in use — unloading netmanager wedged
pupu's jackd and the bridged reference was unusable for a transfer function. This stream is
ac2's own, over the connection it already secures, and touches no jackd.

## 4. Clocks across hosts (N4)

Every daemon already stamps each capture block with its system wall time. It reports in its
state, per device clock:

- the sample clock's rate against the system clock (`48 000.3 Hz`, `+6 ppm`), from the
  block stamps over the last minute — the quantity the drift model needs;
- whether the system clock is disciplined, **as the OS states it**: on Linux the
  `adjtimex` status (`STA_UNSYNC` clear = synced; the maximum error estimate `maxerror` in
  µs); on macOS and Windows *not stated*. The daemon does not guess at PTP versus NTP: a
  PTP-disciplined clock reports a `maxerror` of microseconds, an NTP one of milliseconds,
  and the number is what the operator reads.

The app's Connection page shows per rig *clock: 48 000.3 Hz (+6 ppm) · system clock synced
±15 µs* and, for each pair of rigs with a stream between them, *pupu → ketunkolo: offset
−0.21 ms · drift 2.1 ppm*. A cross-rig measurement's delay readout states its basis
(*clocks ±15 µs*); when a host's clock is unsynced or states ±ms, the delay is shown with
that uncertainty and the transfer function's phase is marked the way an independent-epoch
trace is today (decision 8b): the magnitude stands, the absolute time does not.

With both hosts on PTP the sub-sample alignment the loopback gives on one host is within
reach across hosts: a 15 µs bound is under a sample at 48 kHz.

## 5. Discovery and pairing (N5)

Browse lists every rig on the network; each is connected and paired on its own, as today.
The app remembers connected rigs (key, last address, name) and reconnects them at start;
a rig not found stays as a greyed tab until it answers or is closed. Names come from the
rig and are display only (`ac2-discovery` already trusts nothing).

## 6. What stays single

- A session is one daemon's. Remote inputs make it reach across; it never lives in two.
- Settings (how the operator reads) are the app's; rig settings (rig monitor, OSC targets,
  system max level) are each rig's, edited on its tab.
- Undo is per rig (edits are daemon edits). Layout changes are not undone today and stay so.
- Cross-rig math channels are offered like any other math: both operands are traces or live
  curves the app already has; the daemon doing the math is the measurement's.

## 7. What is and isn't claimed (N6)

- Two rigs' measurements in one layout are as good as each rig's: nothing is lost by
  showing them together.
- A cross-rig measurement is as good as the two clocks' agreement and says so; the app never
  shows a cross-rig delay without its clock basis.
- Audio between daemons rides the LAN; a lost block is a marked gap in the measurement's
  input (as a device xrun is), not silently interpolated.
- No daemon plays a stimulus for another's measurement; §3 says where a measurement runs.

## Stages

1. **Many rigs, tabs, mixed panes** — `App { rigs, active, layout }`, `RigId`, tagged
   events, key routing rule, tabs, pane sources with `RigRef`, picker per rig, layout prefs
   and session pane references, stimulus chips, Shift+Escape, Connect… adds a rig, remembered
   rigs. No wire change. Tests: the app's e2e tests start two fake daemons.
2. **Clock reporting** — daemon state gains the clock lines of §4 (`PROTO_VERSION` bump);
   Connection page shows them; `ac2 status` prints them.
3. **Remote inputs** — `audio.subscribe` stream, `RemoteInput` in the session config,
   B's input setup read at open, the drift model on wall stamps, the Audio & I/O page's rig
   headings, the where-it-runs refusal (`PROTO_VERSION` bump).

Open before stage 1: the tab-switch key (everyday keys plain / Shift / Alt; Alt+←/→ size
panes; Ctrl chords are eaten on the operator's desktop), and whether a tab bar or the top
bar's session line carries the rig name.
