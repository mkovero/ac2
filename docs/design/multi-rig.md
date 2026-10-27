# Many rigs in one app: tabs, mixed panes, remote inputs, clocks across hosts

One app connects to any number of daemons. Each is a full rig — session, measurement tree,
stimulus, calibrations — exactly as the single daemon is today; nothing is a watered-down
"viewer" mode. Panes may show any rig's measurement side by side, and (stage 3) a
measurement on one rig may take a microphone or line input from another. Decisions N1–N6 in
`open-questions.md`. Revised 2026-10-23 after an independent review: the state refactor is
the bulk of stage 1, pane references are a wire change, and cross-host timing rests on
backend capture timestamps and resampling, not on the fan-out's wall stamp.

Operator's framing (2026-10-23): keep every connection functionally identical to the first;
switch between them; by default they are sub-sessions with tabs; another daemon's channels
can go into a measurement. Different magic states would confuse, and there is no reason to
cripple the other rigs. The math is the daemon's; the app renders streams. Both rigs in use
(pupu, ketunkolo) take their system clock from PTP.

## What the operator gets

- **Connect…** adds a rig: browse (`_ac2._tcp`, network mode) or a typed address, paired per
  rig as today. The embedded daemon is a rig like any other (*this computer*).
- **Rigs in the top bar**: a drop-down after the session line names the active rig and
  lists the others; a click switches. It is gone in the stage view and full screen like the
  rest of the top bar. There is **no tab-switch key** (operator, 2026-10-23): clicking is
  enough, and slots do the keyboard's part — a slot (K7, Ctrl+1 … 9 capture, 1 … 9 show)
  remembers its measurement's rig, and showing a slot of another rig makes that rig active,
  so 1 … 9 step between measurements across rigs. The active rig is the one the single-rig
  keys talk to: Space arms *its* stimulus, Enter opens *its* session,
  the tree is *its* tree, Settings' Audio & I/O is *its* devices. Switching rigs switches all
  of that; nothing else changes. With more than one rig connected the arm line and the
  stimulus banner **name the rig** (*arm pupu's stimulus · Space*): sound must never leave a
  system the operator did not mean.
- **Panes are the app's**, not a tab's: the pane tree stays when the tab changes, and a pane
  shows a measurement, input set or pair of any connected rig. A pane's title names the rig
  when it is not the active one (*pupu · Stage*). Pick and drag work across rigs: the picker
  lists every rig's measurements under a heading per rig.
- **Remote inputs** (stage 3): a rig's session may include a mic or line input of another
  rig (*ketunkolo · in 3*). It appears on the Audio & I/O page under its rig, with its name,
  kind and calibration as that rig states them, and goes into measurements like a local
  input — a new measurement simply offers the other rigs' channels.
- **Clocks**: the Connection page says, per rig, how its sample clock runs against its system
  clock and what the OS says about that clock; between two rigs, the measured relation of
  their capture streams once a stream exists. A cross-rig measurement states what its time
  axis rests on.

The two rigs in use: a Programme pane with ketunkolo's console-feed loudness beside pupu's
SPL meter at the place, in one layout, on one screen (or one Pi).

## 1. One state per rig, one layout per app (N1)

Today's `AppState` (`ac2-ui/src/state.rs`) mixes two things: what belongs to a rig
(connection, session, tree, selection, stimulus, measurement-bound maps) and what belongs to
the app (layout, view, navigation, prefs, toasts, rasteriser, pane area). Stage 1 splits
it:

```rust
pub struct App {
    rigs: Vec<RigState>,          // in tab order; at least the embedded or first rig
    active: RigId,                // the tab shown
    layout: PaneTree,             // panes with cross-rig sources (§2)
    settings: Settings,           // app-wide (how the operator likes to read)
    // view, nav, toasts, rasteriser … (app-level as today)
}
```

`RigId` is a daemon identity the hello reply carries: a random id the daemon makes once and
keeps in its config directory beside `server.key`. It cannot be the CURVE key: a local
(ipc) or embedded daemon runs without CURVE and has none. The id is stable across
reconnects, address changes, renames and a move from local to network mode; pairing still
keys on the CURVE key, and the Connection page shows that key's fingerprint as today.
Display name from the hello (and discovery), re-read on each connect.

**Ids are per daemon.** `MeasId`, `TraceId`, `PairId` and frame topics collide across rigs,
and the app keys a dozen maps by them today (bar origins, traces, edits, peaks,
spectrographs, SPL holds, selection…). Every such key becomes `(RigId, MeasId)` (or lives
inside the `RigState` that owns it), every reducer event names its rig, and the link thread
tags what it delivers with its `RigId`. This namespacing and the `AppState` split **are**
stage 1; the tabs are the small part.

Keys are routed by **one rule**: a key scoped to a pane goes to the pane's rig (**R** on a
Programme pane of ketunkolo resets ketunkolo's max and overs; **E** picks the phase reference
of that pane's transfer); every other key goes to the active tab's rig. App-level toggles
(**T** chrome, **W** / **F** view) are not rig commands and are unchanged. No key ever acts
on a rig the operator is neither looking at (tab) nor pointing at (focused pane).

A rig that drops out keeps its tab and its panes: panes grey with *pupu unreachable ·
retrying* and the connection retries as today. Closing a tab closes that connection (its
stimulus stopped and lease released first, `QUIT_GRACE`); its panes stay in the layout
saying *pupu not connected* until re-connected or re-pointed. Quit stops every rig's held
stimulus in parallel, each within `QUIT_GRACE`.

### Stimulus awareness

Stimulus runs per rig under its own lease. The top bar shows the active rig's stimulus as
today and names the rig when more than one is connected. **Shift+Escape stops every rig's
stimulus** and is in stage 1 with the tabs: the moment two rigs can play, a stop that needs
a tab switch first is not the universal stop decision K1 promises. A chip per *other* rig
whose stimulus this app holds (*▶ pupu −20 dBFS*) follows in stage 2: sound coming out of a
system you are not looking at must not be silent in the UI.

## 2. Pane sources and the P1 ruling (N2)

```rust
pub struct PaneSource { pub rig: RigRef, pub content: PaneContent }   // PaneContent: programme-meter.md §6
pub enum RigRef {
    /// App prefs only: the pane follows the active tab (the single-rig meaning).
    Active,
    /// Session files only: the rig this session belongs to, whatever tab is active at load.
    ThisRig,
    /// A pinned rig, by its RigId (§1; never the CURVE key, which a local rig lacks and a
    /// rig moving to network mode gains); the name is for the operator when it is absent.
    Rig { id: RigId, name: String },
}
```

`Active` is the default in the layout prefs. In a **session file** a pane source is
`ThisRig` or `Rig`, never `Active`: a session is one daemon's, and "the active tab" at load
time could be another rig's ids. Enforced where each side can: the daemon refuses `Active`
in `meas.layout` by name (*a measurement's layout names its own rig or another, never the
active tab*); the app maps `ThisRig` to the session's rig at load and writes `ThisRig` for
that rig on save. A `Rig { .. }` pane's content is another daemon's ids, which this daemon
cannot check: it stores them opaque and the app resolves them. The picker and a drop set
`Rig` when the source is not the active rig.

Pane sources live in the measurement's layout (`Measurement.layout`, sent by `meas.layout`,
saved in the session: decision P1), so adding `RigRef` and `PaneContent` is a **`PROTO_VERSION`
and session format bump**, in stage 1.

**P1 across rigs.** P1 says choosing a measurement row brings its own panes and stores the
panes edited while it is chosen. With rigs, a measurement's layout **stores only `ThisRig`
panes**: a pane pinned to another rig is never written into a measurement's layout (it
would come back as a duplicate of the pinned pane already on screen); it lives in the app's
layout prefs. Bringing rig A's measurement replaces the panes whose source is `Active` or
`ThisRig` of A and leaves pinned panes where they are. The operator who put ketunkolo's
loudness beside pupu's meter keeps it when stepping through pupu's measurements. A
Programme pane opened from a pair row (a child row; no measurement chosen) is likewise the
app's: child rows never store panes (list click model), so it is saved in the layout prefs
only.

Layout prefs save `Rig` by id and name. Loading a session whose panes reference a rig this
app has not connected shows those panes as *ketunkolo not connected* with the content's
name, so the layout survives and says what is missing.

## 3. Remote inputs (N3, stage 3)

A rig's session includes a mic or line input of another rig by subscribing to it: daemon A
asks daemon B for channel `c` and B streams its capture of `c` to A. The reference is the
session's `LoopbackRoute` on A's own device and is never remote (the loopback is a loopback
because it returns on the same converter), so a remote input is a measurement or line input
only; nothing has to be refused about where a measurement runs.

What stage 3 needs, none of it designed yet beyond this list:

- **Capture timestamps from the backend**, not the fan-out's wall stamp: the fan-out stamps
  a block when its thread receives it (scheduling and buffer-period jitter, milliseconds),
  which gives a drift slope over a minute but no offset. Backends can state the capture
  time of a block's first sample: JACK cycle times, Core Audio `mHostTime`, ASIO
  `systemTime`. Each must be measured against the loopback on the rigs in use before any
  cross-rig offset is shown; and each interface's input latency (ADC + driver, device
  specific) remains a constant the clocks do not remove — it is stated per rig as the
  backend reports it, or the cross-rig delay is shown as *offset unknown*, never as a number
  it is not.
- **Resampling.** Two interfaces run on two crystals (48 000.0 vs 48 000.3 Hz is 6 ppm:
  0.3 ms drift every 50 s, 108° at 1 kHz). A measurement that combines samples from both
  must resample the remote stream onto A's capture clock continuously, the ratio tracked
  from the timestamps — an asynchronous sample-rate converter in A's block pipeline, which
  does not exist today (`multi-device.md` §6 only warns about drift and refuses the
  reference). The drift model of §5 there supplies the ratio; the converter is new.
- **A buffer policy**: A delays its local blocks so that local and remote meet at one sample
  index; the delay is the network jitter bound plus one block and is added to every live view
  of that session. Stated on the Audio & I/O page (*remote inputs add 12 ms*).
- **Daemon as client.** A opens a CURVE connection to B: A needs a client key and B must
  have paired it. Today pairing is app ↔ daemon; a daemon ↔ daemon pairing (the app that
  holds both asks B to admit A's key) is a new flow.
- **Loss.** A PUB stream drops at its high-water mark; a dropped block is a marked gap in
  the remote input (as a device xrun is), never interpolated. The stream is `f32` blocks,
  sample-indexed in B's capture clock, each with B's backend capture timestamp; 48 kHz mono
  is 1.5 Mbit/s a channel, stated on the Connection page.
- **Session**: `SessionConfig.input_channels` grows `RemoteInput { rig, channel }`. The remote
  input's `InputSetup` (kind, mic, curve) and calibration are **B's**, read from B at open
  and re-read when B says they changed, never copied into A's store — the mic is on B's input
  and B is where it was calibrated. A's Audio & I/O page shows it under a *ketunkolo*
  heading with B's row text; editing it says *set on ketunkolo*.

Why not netjack: it was tried (2026-10-11) and failed in use — unloading netmanager wedged
pupu's jackd and the bridged reference was unusable for a transfer function. This stream is
ac2's own, over the connection it already secures, and touches no jackd.

Precondition for designing stage 3 further: a timestamp and latency study on pupu and
ketunkolo (backend timestamp against loopback, over an hour, both hosts), written up in
`multi-device.md` §7 (input-vs-input, designed never built) which this stage would build.

## 4. Clocks across hosts (N4)

Each daemon reports in its state, per device clock, what it can measure and what the OS
says, and infers nothing:

- the sample clock's rate against the system clock as a deviation in ppm (`+6 ppm`), from
  the block wall stamps over the last minute — the rig monitor already speaks ppm, and
  `48 000.3 Hz` beside it would be the same number twice;
- the system clock's own status **as the OS states it**: on Linux the `adjtimex` status
  word (`STA_UNSYNC` clear or set) and its `maxerror` estimate; on macOS and Windows *not
  stated*. `maxerror` grows between updates (about 500 µs per second until the discipline
  daemon resets it), so it is shown as what it is — the OS's current bound — and is never
  turned into a "PTP" or "NTP" label. What a PTP-disciplined host actually reports is read
  on pupu and ketunkolo before the Connection page promises any figure.

The Connection page shows per rig *clock: +6 ppm · system clock synced, OS bound ±0.4 ms*
and, once a remote stream exists (stage 3), per pair of rigs the measured relation of their
capture streams. A cross-rig measurement's delay readout states its basis in words; when a
host's clock is unsynced, or no backend timestamp has been validated for it, the delay is
*offset unknown* and the transfer function's phase is marked the way an independent-epoch
trace is today (decision 8b): the magnitude stands, the absolute time does not.

## 5. Discovery and pairing (N5)

Browse lists every rig on the network; each is connected and paired on its own, as today.
The app remembers connected rigs (key, last address, name) and reconnects them at start;
a rig not found stays as a greyed tab until it answers or is closed. Names come from the
rig and are display only (`ac2-discovery` already trusts nothing).

## 6. What stays single

- A session is one daemon's. Remote inputs make it reach across; it never lives in two.
- Settings (how the operator reads) are the app's; rig settings (rig monitor, OSC targets,
  system max level, programme-meter ballistic) are each rig's, edited on its tab.
- Undo is per rig (edits are daemon edits). Layout changes are not undone today and stay so.
- Cross-rig math channels are offered like any other math: both operands are traces or live
  curves the app already has; the daemon doing the math is the measurement's.

## 7. What is and isn't claimed (N6)

- Two rigs' measurements in one layout are as good as each rig's: nothing is lost by
  showing them together.
- A cross-rig measurement is as good as the two clocks' agreement *and* the validated
  backend timestamps *and* the stated input latencies, and says so; the app never shows a
  cross-rig delay without its basis, and shows *offset unknown* until all three exist.
- Audio between daemons rides the LAN; a lost block is a marked gap, never interpolated.
- No daemon plays a stimulus for another's measurement; the reference is never remote.

## Before stage 1

This note was written at PROTO 66 (2026-10-23). What landed since and touches it, to fold in
before building (then a fresh design review, as every design here gets):

- **Pane layout on the wire:** `LayoutPane` gained `programme_view` (Meters / Goniometer,
  programme-meter.md §6.3); `PaneSource { RigRef, PaneContent }` wraps it with the rest.
  A pair's programme meter opens two panes in one undo step (`state/programme_views` tests):
  both panes belong to one rig.
- **Per-connection state already reset on connect:** `ConnEvent::Connected` in
  `crates/ac2-ui/src/state/link.rs` clears the device list (another daemon has other ports).
  In the §1 split, the device list, input setup and session dialog are each rig's.
- **Sessions without reference or stimulus** and **mic pairs** (`two_input_refusal` in
  `crates/ac2-ui/src/state/new_meas.rs`): a rig may meter only, which a remote input (§3)
  must not assume otherwise.
- **JACK spare inputs** (`OpenSession.spare_ports`, rig file 6+): routes into a rig's spare
  ports are that host's JACK graph, not something another rig can patch.
- **Frame pacing** (`crates/ac2-ui/src/pace.rs`): one pacer per app, every rig's wake goes
  through it.
- Current versions: PROTO 71, session format 35, rig file 7 (`crates/ac2d/src/rig/v6.rs` is
  the last upgrade step).

## Stages

1. **Many rigs, tabs, mixed panes** — the `AppState` split and id namespacing (§1),
   `RigId` in the hello reply, tagged events, key routing rule, tabs, `PaneSource { RigRef,
   PaneContent }` in `Measurement.layout` and the layout prefs with the P1 storage rule and
   the daemon's refusal of `Active` (`PROTO_VERSION` and session format bump), picker per
   rig, arm line and banner naming the rig, Shift+Escape stopping every rig, Connect… adds a
   rig, remembered rigs. Tests: the app's e2e tests start two fake daemons; a session saved with
   `ThisRig` panes loads right on either tab, and a `Rig` pin resolves by id after the
   pinned daemon reconnects on another address.
2. **Clock reporting and stimulus chips** — daemon state gains the clock lines of §4
   (`PROTO_VERSION` bump), Connection page shows them, `ac2 status` prints them; other-rig
   stimulus chips in the top bar.
3. **Remote inputs** — after the timestamp and latency study; redesigned then on the list
   in §3.

Settled 2026-10-23 (operator): no tab-switch key; a top-bar drop-down (hidden in the stage
view) carries the rig; slots carry their rig and showing one switches to it.
