# Many rigs in one app: tabs, mixed panes, remote inputs, clocks across hosts

One app connects to any number of daemons. Each is a full rig — session, measurement tree,
stimulus, calibrations — exactly as the single daemon is today; nothing is a watered-down
"viewer" mode. Panes may show any rig's measurement side by side, and (stage 3) a
measurement on one rig may take a microphone or line input from another. Decisions N1–N6 in
`open-questions.md`. Refreshed against PROTO 71 and reviewed again 2026-10-27. Revised 2026-10-23 after an independent review: the state refactor is
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
  enough. Slots (K7, Ctrl+1 … 9 capture, 1 … 9 show) are each rig's — a slot is a field of
  the daemon's trace (`TraceMeta.edit.slot`), so every rig has its own 1 … 9 — and act on
  the active rig's slots like every other rig key (operator, 2026-10-27); the drop-down is
  the only rig switch. The active rig is the one the single-rig keys talk to: Space arms
  *its* stimulus, Enter opens *its* session, the tree is *its* tree, 1 … 9 are *its* slots,
  Settings' Audio & I/O is *its* devices. Switching rigs switches all of that; nothing else
  changes. With more than one rig connected the arm line and the
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
keeps. It cannot be the CURVE key: a local (ipc) or embedded daemon runs without CURVE and
has none (`server.key` is only loaded in network mode). The daemon loads it at every start,
in every mode: `DaemonConfig.rig_id: RigIdSource::{File(PathBuf), Ephemeral}`. `ac2d` uses
`ac2_paths::rig_id()` (`<config>/rig-id`, created if absent); the app's embedded real-audio
daemon uses `ac2_paths::app_rig_id()`, separate from the system daemon's on the same
machine as `app_autosave_dir()` is from `autosave_dir()` — both can be connected at once,
and two rigs must never share an id; the embedded fake rig is `Ephemeral` (random per
start; nothing of it persists, as its calibrations and rig settings do not). The id is
stable across reconnects, address changes, renames and a move from local to network mode;
pairing still keys on the CURVE key, and the Connection page shows that key's fingerprint as
today.

The hello reply (`Welcome`, `ac2-proto/src/ctrl/reply.rs`) gains `rig_id: RigId` and
`name: String`. The name is the `--name` that network mode advertises, defaulting to
*ac2 on <hostname>* in every mode; the embedded daemon's is *this computer*. It is re-read
on each connect and is display only. `FakeOptions` (`ac2-client/src/fake.rs`) gains both, so
tests can start two distinguishable fake daemons; `ConnEvent::Connected` and
`ConnState::Connected` carry them.

**Ids are per daemon.** `MeasId`, `TraceId`, `PairId` and frame topics collide across rigs,
and the app keys a dozen maps by them today (bar origins, traces, edits, peaks,
spectrographs, SPL holds, selection…). Every such key becomes `(RigId, MeasId)` (or lives
inside the `RigState` that owns it), every reducer event names its rig, and the link thread
tags what it delivers with its `RigId`. This namespacing and the `AppState` split **are**
stage 1; the tabs are the small part.

**Which field is whose.** Of today's `AppState` (`ac2-ui/src/state.rs`):

- *The rig's* (`RigState`): `conn`, `mirror`, `data`, `traces`, `tf_display`, `advice`,
  `bar_from`, `spec_glide`, `spec_pacing` (by topic), `edits`, `peaks`, `peak_tracks`,
  `spectrographs`, `spl_hold`, `spectrum_running`, `hidden_meas`, `collapsed`; the selection
  and the P1 bookkeeping (`selected`, `selected_trace`, `selected_live`, `standin`,
  `lost_selection`, `own_panes`, `lost_own_panes`, `own_panes_due`, `layout_queue`,
  `pending`, the `*_on_create` names, `choose_when_open`, `open_session_when_empty`);
  `stimulus`, `sweep`, `armed_with`, `stim_device`, `finder`, `embedded_autosave`;
  `devices`, `devices_for`, `preview_sent_s`; `input_pick` (input numbers are a daemon's);
  `link_wants`; `leq`'s history, spans and logs; `ProgrammeUi`'s flying peaks and rings
  (keyed by input channel, which collide across rigs).
- *The app's* (`App`): `layout`, `pane_area`, `pane_levels`, `spectrum_hold` and every
  other map by `PaneId` (plate hover, pane export, `leq`'s strips and cursor), `view`, `nav`,
  `theme`, `prefs`, `prefs_dirty`, `export_dir`, `local_zone`, `build_id`, `overlay`,
  `help_scroll`, `help_page`, `toasts`, `notices`, `rasterizer`, `now_s`, `quit`,
  `fullscreen`, `panes_only`, `swallow_text`, `pane_auto`, `client_key` (one CURVE identity
  for every rig), `want_connect_dialog`, `tree_reveal`, `settings_reveal`,
  `settings_page_last`, undo.

**Windows.** `overlay` is the app's, but most windows act on one rig: Settings (its session
dialog, connection and rig pages), the measurement forms, delay pick, EQ, Sessions, Delete,
Choose, new log. A window opens for the active rig and remembers it; the rig drop-down is
disabled while a window is open (K9: the window owns the input), so a window never lands on
another rig's state.

**One link per rig.** Today the app has one `Conn` (`app.rs`), one `pump_link`, `Msg::Conn`,
and the reducer returns `Request`s for that link; `wanted_topics` / `sync_link`
(`link_wants.rs`) run over one mirror. `Conn` is already self-contained (own thread, runtime,
inbox, stimulus task) and the frame pacer (`pace.rs`) is shared through an `Arc`, so per-rig
links are cheap: `RigState` owns its `Conn`; `Msg::Conn(RigId, Box<ConnEvent>)`; the reducer
returns `(RigId, Request)`; `wanted_topics` / `sync_link` run per rig over the whole layout
(a pane of rig B on screen while A is active still wants B's topics); every rig's wake goes
through the one pacer, and data arriving from any rig counts as data. `ConnEvent::Connected`
already clears the device list (another daemon has other ports); with the split it resets
that rig's state only.

**Remembered per device.** `UiPrefs.sessions` and `outputs` are keyed by device id strings,
and `jack:system` exists on both rigs in use: they are keyed by `(RigId, device)`.

Keys are routed by **one rule**: a command that resolves to a pane — the focused pane's
scope, or a global command that acts on the most recently used pane of a kind
(`Layout.mru`) — acts on that pane's rig (**R** on a Programme pane of ketunkolo resets
ketunkolo's max and overs; **E** picks the phase reference of that pane's transfer); keys in
the Inputs scope (`input_pick`) and every other command act on the active tab's rig. App-level toggles
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
a tab switch first is not the universal stop decision K1 promises. The key is already bound
(`STOP_ANYWHERE`, handled before any window) and documented; stage 1 fans the stop out to
every rig's link — no new key. A chip per *other* rig
whose stimulus this app holds (*▶ pupu −20 dBFS*) follows in stage 2: sound coming out of a
system you are not looking at must not be silent in the UI.

## 2. Pane sources and the P1 ruling (N2)

`LayoutPane` (`ac2-proto/src/model/layout.rs`) already carries `content:
Option<PaneContent>` (`None`: the measurement whose layout this is; programme-meter.md §6)
and `programme_view`. It gains the rig; there is no wrapper type:

```rust
pub struct LayoutPane { /* kind, views, chrome, content as today */ #[serde(default)] pub rig: RigRef }
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

The serde default is `ThisRig`; `Active` is the default in the layout prefs. In a
**session file** a pane's rig is `ThisRig` or `Rig`, never `Active`: a session is one
daemon's, and "the active tab" at load time could be another rig's ids. Enforced where each
side can: `PaneLayout::check()` (the daemon's refusal point for `meas.layout`, and for the
rig file's meter layouts) refuses `Active` by name (*a measurement's layout names its own
rig or another, never the active tab*) and `Rig { .. }` with `content: None` (another rig's
pane must name what it shows); the app maps `ThisRig` to the session's rig at load and writes `ThisRig` for
that rig on save. A `Rig { .. }` pane's content is another daemon's ids, which this daemon
cannot check: it stores them opaque and the app resolves them. The picker and a drop set
`Rig` when the source is not the active rig.

Panes live in the measurement's layout (`Measurement.layout`, sent by `meas.layout`, saved
in the session: decision P1) and in the rig file's meters (`RigMeter.layout`), so adding
`rig` is a **`PROTO_VERSION` (71 → 72), session format (35 → 36) and rig file (7 → 8,
`rig/v7.rs` the upgrade step) bump**, in stage 1.

On the app side the pane's `View` gains `rig: RigRef` mirroring `LayoutPane.rig`. A pair's
inputs (`Metered::Pair { left, right }`) are one daemon's channel numbers: both panes a pair
opens (Goniometer and Meters, one undo step) take the rig of the Inputs list the pair was
picked from, and the picker's per-rig headings list `Meter` rows too.

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

Layout prefs (`ui.toml`, `PaneEntryFile`) keep a pane's content by name and inputs as
today, and gain `rig: Option<{ id, name }>` (absent: `Active`); a `Rig` pane's content
resolves by name once the pinned rig is connected, as a measurement gone and back does
today (`park_gone_meas`). Loading a session whose panes reference a rig this
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

- **Rigs that only meter.** A session may have no reference or stimulus, and two inputs
  may be a mic pair (`two_input_refusal`, `state/new_meas.rs`): a remote input must not
  assume the receiving rig has a loopback or plays anything.
- **JACK spare inputs** (`OpenSession.spare_ports`) are that host's JACK graph; another rig
  can neither see nor patch them, and a remote input is never a route into them.

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
- Undo is the app's history of pane edits (`state/undo.rs`) and stays one history across
  every rig's panes; what a daemon does is not undone, as today.
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

## Stages

1. **Many rigs, tabs, mixed panes**, in four parts that each land green:
   - **1a — wire and files** (no UI behaviour change): `RigIdSource` and its files,
     `Welcome { rig_id, name }`, `--name` in every mode, `LayoutPane.rig` and `check()`'s
     refusals, PROTO 72 / session format 36 / rig file 8, `FakeOptions`, protocol fixtures
     and `WIRE_LOCK`.
   - **1b — `App { rigs, active }` with exactly one rig**: every rig field of §1 moved into
     `RigState`, `Conn` per rig, `Msg::Conn(RigId, …)`, `(RigId, Request)`, per-rig
     `sync_link`, prefs keyed by `(RigId, device)`. Snapshot- and behaviour-neutral: every
     existing test passes unchanged.
   - **1c — many rigs**: Connect… adds a rig, the drop-down, remembered rigs and reconnect
     at start, greyed tabs and panes (*unreachable · retrying*, *not connected*), arm line
     and banner naming the rig, Shift+Escape fanned out, quit stopping every rig in
     parallel, windows bound to their rig.
   - **1d — cross-rig panes**: `View.rig`, picker and drag across rigs, pane titles naming
     another rig, P1 storing only `ThisRig` panes, prefs `rig` entries and their resolution
     on reconnect, `ThisRig` mapped at session load and written at save, pair panes.

   Tests: namespacing is proven in the daemon-free reducer tests (`state/tests.rs`) feeding
   two rigs' mirrors with colliding `MeasId`s, topics and input numbers; the app's e2e tests
   start two fake daemons (`FakeDaemon::start` is per instance) for connect, reconnect and
   the stop; a session saved with `ThisRig` panes loads right on either tab, and a `Rig` pin
   resolves by id after the pinned daemon reconnects on another address.
2. **Clock reporting and stimulus chips** — daemon state gains the clock lines of §4
   (`PROTO_VERSION` bump), Connection page shows them, `ac2 status` prints them; other-rig
   stimulus chips in the top bar.
3. **Remote inputs** — after the timestamp and latency study; redesigned then on the list
   in §3.

Settled 2026-10-23 (operator): no tab-switch key; a top-bar drop-down (hidden in the stage
view) carries the rig. Revised 2026-10-27 (operator): slots are each rig's and act on the
active rig; the drop-down is the only rig switch.
