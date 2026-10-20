# ac2 user guide

ac2 is a live dual-channel FFT analyzer for tuning sound systems: transfer functions with
coherence, a delay finder, sweep measurements with harmonic distortion, spectrum and RTA, a
calibrated SPL meter with rolling Leq windows, stored traces, sessions and autosave. This
guide explains the concepts and the everyday workflow. Installing: [install.md](install.md).
Scripting: [protocol.md](protocol.md), the normative description of what the daemon speaks.

| I want to… | Go to |
|---|---|
| connect the audio interface and open a session | [Audio I/O](#audio--io) |
| wire the loopback so transfer functions work | [Reference wiring and loopback](#reference-wiring-and-loopback) |
| play pink noise safely, and stop it | [Stimulus](#stimulus) |
| see magnitude, phase and coherence of a speaker | [Transfer measurement](#transfer-measurement) |
| time-align a speaker | [Delay finder](#delay-finder) |
| align two systems in their overlap (sub and main, a delay tower) | [Alignment](#alignment-delay-and-polarity-of-two-systems) |
| keep a curve to compare against | [Traces and slots](#traces-and-slots) |
| measure response and harmonic distortion with a sweep | [Sweep measurement](#sweep-measurement-response-and-harmonic-distortion) |
| read dB SPL and Leq | [Calibration and SPL](#calibration-and-spl), [SPL meter](#spl-meter), [Leq windows and limits](#leq-windows-and-limits) |
| save the work, or get it back after a crash | [Sessions](#sessions), [Autosave](#autosave) |
| record the inputs and measure them again later | [Recording and replay](#recording-and-replay) |
| look up a key | [Keyboard map](#keyboard-map) |
| script it | [Command line](#command-line), [protocol.md](protocol.md) |

## How the pieces fit

```
audio interface ── ac2d (daemon) ──┬── ac2-ui  (the app: plots, keys)
                                   ├── ac2     (CLI: setup, scripting, --json)
                                   └── your own client (ZeroMQ + msgpack, docs/protocol.md)
```

The **daemon** owns the audio interface and does all measuring. The app and the CLI are
clients; several can be connected at once (a laptop at FOH, another at a delay tower) and
see the same measurements. Locally the daemon is reachable only by your user on Linux and
macOS (sockets in your runtime directory); on Windows it listens on `127.0.0.1:47820`
without encryption, so any program on the same computer can reach it. Across the network it
requires pairing ([install.md](install.md#remote-use-foh--stage)).

A **session** is the open audio stream: one device, a sample rate, a buffer size and the
inputs to capture. **Measurements** run on it: transfer functions (`tf`), spectrum, RTA,
SPL meters and sweeps. In the app, every setting lives in **Settings** (**Ctrl+P**, the ⚙ in
the top bar); **Shift+O** opens its Audio I/O page; **Ctrl+M** (*New measurement…*, also
the last row of every pane's list) lists every kind — transfer, spectrum, RTA, SPL meter,
sweep, math channel — with what each measures, and opens the chosen kind's settings (created,
started and selected on **Enter**, in a pane of its own); the palette (**Ctrl+K**) also has
*New transfer…*, *New spectrum…*, *New RTA…*, *New SPL meter…* directly, and
*Close audio I/O*; **Delete** (or **Backspace**) deletes the
selected measurement after asking. Until there is a session or a measurement, the transfer
pane says which comes next. The CLI does the same with the same defaults:

```sh
ac2 session open --backend jack --in 1-4                     # Linux: the JACK server
ac2 session open --backend coreaudio --device "<name>" --in 1-4 --rate 48khz   # macOS
ac2 session open --backend asio --device "<driver>" --in 1-4              # Windows
ac2 session open --backend coreaudio --device "<mic>" --out-device "<speakers>" --in 1   # two devices
ac2 meas new tf --ref 1 --meas 2 --name main-l
ac2 meas new tf --ref 1 --meas 3 --name sub
ac2 meas new rta --input 2 --name rta
ac2 meas start main-l
```

### Log files

The app and a daemon started by hand log sessions opened, devices lost, capture
discontinuities, *audio arrives in bursts* and errors to their terminal and to a file —
attach it to a bug report:

| | Linux | macOS | Windows |
|---|---|---|---|
| directory | `~/.local/state/ac2` (`$XDG_STATE_HOME/ac2`) | `~/Library/Logs/ac2` (Console.app lists it) | `%LOCALAPPDATA%\ac2\logs` |

`ac2-ui.log` is the app's (with the daemon it hosts), `ac2d.log` a standalone daemon's. Each
start moves the previous file to `ac2-ui.log.1` / `ac2d.log.1`, so a bad run survives one
restart; a file that reaches 16 MiB during a run is moved there too and a fresh one started,
so a weeks-long run keeps at most 32 MiB of log. A panic is logged with its thread and a
backtrace before the program exits (release builds stop on any panic; under systemd the unit
restarts it: [Watchdogs](#watchdogs-under-systemd)). A daemon under systemd (`systemctl --user`) logs to the journal
(`journalctl --user -u ac2d`). `RUST_LOG` chooses what is logged (default `warn,ac2=info`;
`RUST_LOG=debug` for everything); `AC2_LOG_DIR` moves the files. The app's notifications are
not in the file; *Notifications…* (Shift+E) lists them ([Notifications](#notifications)).

### Audio backends

The backend is always named. Each platform has one real backend:

| Platform | Backend | Notes |
|---|---|---|
| Linux | `jack` | a JACK2 server, or PipeWire through pipewire-jack; there is no ALSA backend. Rate and buffer are the server's. |
| macOS | `coreaudio` | Core Audio, the HAL directly: one callback per device serves its inputs and outputs on one exact sample index, and the session states the device's latency. Two devices (a USB mic beside the built-in output) run one callback each, on two clocks; an aggregate device made in Audio MIDI Setup is one device. With no `--rate`, the device's current rate (a rate is set only when asked for, since other apps share it); with no `--buffer`, a short buffer of about 20 ms (1024 frames at 48 kHz, within what the device allows). Only the input streams a session's channels are on are read. |
| Windows | `asio` | ASIO drivers, one device per installed driver, inputs and outputs on one clock. With no `--rate` / `--buffer`, the driver's current rate and preferred buffer. A device without its own ASIO driver goes through [FlexASIO](https://github.com/dechamps/FlexASIO); with no ASIO driver installed, the backend is listed as unavailable with that pointer. |
| any | `fake` | a simulated rig for trying things out; never chosen for you |

**macOS: microphone access.** macOS lets a program read any audio input only once the
user allows it, so ac2 asks before it opens an input (the first time Settings shows the
input meters, usually). While the prompt goes unanswered, or once access is refused, ac2
says so in words and nothing is opened: allow ac2 in **System Settings › Privacy & Security
› Microphone**. macOS asks on behalf of the program that started ac2: the app for itself and
the daemon it hosts, Terminal for `ac2d` or `ac2 selftest` run there. A daemon started by
launchd or over ssh gets no prompt; allow its app by hand first ([install.md](install.md)).

Should a host still deliver audio in lumps, the daemon log says so once a minute (*audio
arrives in bursts on …*) with the device and buffer to change.

In the app's connect dialog, *Simulated rig* starts ready to measure (audio I/O open, a
transfer measurement "demo" running); *This computer's audio* starts with no session.
Whenever the app connects to a daemon with no audio I/O open, the **Sessions** dialog
(**Alt+S**) opens once; a daemon with audio open (a rig, the Pi kiosk) opens nothing. Its
*New session* opens Settings on the Audio I/O page, headed *New session — Audio I/O*, with the
input device focused (a real interface preselected); Enter opens the audio I/O, and the empty
session then offers the New measurement chooser. *Open…* or a recent session opens that one with
the audio I/O the daemon has. Esc leaves an empty app whose transfer pane says *No session open
— Alt+S, then New session*. With no audio I/O, **Ctrl+M** and the *New transfer…*,
*New spectrum…*, *New RTA…*, *New SPL meter…* and sweep entries open the Sessions dialog too.
**Shift+O** still opens the Audio I/O page directly (under its own name), to change the
audio of the session at hand.

A new transfer is named after what it compares: *Reference → M30 FOH* (the measurement
input's name), numbered *Reference → M30 FOH 2* … when that name is taken; *TF n* only when
the input has no name.

### Settings

**Ctrl+P** (palette *Settings…*, or the ⚙) opens every setting in one view below the top
bar, so what drives the speakers stays in sight. Pages are in a sidebar:
**Ctrl+PgUp / Ctrl+PgDn** (or **Alt+←** / **Alt+→**) step, **Alt+1 … Alt+7**
jump (hovering a page names its key), a click picks; Ctrl+P reopens the page last shown,
and closes the view again. Esc
closes the view (a dialog or confirmation over a page first); **Shift+Esc** still stops the
stimulus. Each page says whose its settings are: **this app** (`ui.toml` on this computer)
or **the rig — all clients** (kept by the daemon; every connected app sees a change at
once).

| Page | What | Opened also by |
|---|---|---|
| Audio I/O | backend, input and output device, rate, buffer; inputs and outputs by name, roles, mics, the stimulus outputs, the reference; the **system max level**; Enter opens the audio I/O | **Shift+O** (on the input device), palette *Input setup…* (on the first input), *Stimulus outputs…* (on the stimulus output) |
| Calibration | mics, curves, sensitivity calibrations ([below](#the-calibrations-view)) | palette *Calibrations…* |
| SPL / Leq | the SPL meter's Leq windows, limits and band meter ([below](#leq-windows-and-limits)) | **Shift+L** |
| Recording | how long the record toggle records (this app); where the daemon records (the rig) | |
| Display | theme, key hints, warning toasts, how long the SPL number holds, Leq windows / history strip / bars, what the spectrum and sweep panes show, spectrum glide, distortion unit, coherence placement, resolution marker, level axes | |
| Connection | the daemon, this client's id and key, reconnect, another daemon / pairing; the daemon's mode, mDNS name, authorized clients and refused keys | |

#### System max level

The generator never plays above the rig's maximum (dBFS RMS). The last row of the Audio &
I/O page shows it with its bound and who changed it last:
`−40.0 dBFS · bound −10.0 dBFS (ac2d --max-level)`. Type a level and **Enter**:

- a **lower** level applies at once for every client; a stimulus armed or playing above it
  is stopped;
- a **higher** level asks you to type **raise** first, is refused while anything is armed or
  playing, and never passes the daemon's bound (`ac2d --max-level`, default −10 dBFS).

The daemon keeps the level across restarts. CLI: `ac2 gen ceiling`,
`ac2 gen ceiling -40dbfs`, `ac2 gen ceiling -30dbfs --yes`.

#### Connection

In network mode the page lists the clients the rig accepts (name, key fingerprint) and the
keys it refused lately (fingerprint, address, how often, when).

- **A** on a refused key, then a name, authorizes it; the client connects on its next retry.
- *Authorize a client by its key…* takes a name and the key `ac2 auth show` prints.
- **Delete** twice revokes a client: its requests are refused at once. You cannot revoke
  your own.

### Audio I/O

Nothing on this page is a channel number to type. Rows run in the order you decide them:
device (backend, input and output device), inputs, outputs, the reference (loopback), sample
rate and buffer, then the system max level; **↑/↓** walk them and wrap; **Enter** (or the
*Open audio I/O* button, kept in view at the foot of the page — *Open new session* after
Sessions › New) opens, or reopens, the audio I/O as shown; **Esc** closes without opening.

- **Backend** (**←/→**): every backend the daemon offers; one it cannot use now says why and
  what to do (*PipeWire is running but its JACK library isn't in use: install pipewire-jack …
  or start the daemon with `pw-jack ac2d`*). The simulated rig only appears on a daemon
  started on it.
- **Input device** (**←/→**): the capture device, with *N in / M out · rate · buffer*. Output-only
  devices are not offered.
- **Output device** (**←/→**): where the stimulus plays. It starts as *same device as the input*
  when that device has outputs (one device, one clock), else the system's default output —
  a USB mic has no outputs. With two devices the clocks may differ, and Settings says so: the loopback
  reference keeps the transfer function right, and the loopback monitor measures their
  drift while a stimulus plays. The choice is remembered with the input device's roles.
- **Channels**: one row per input and output, named by the backend where it can (JACK port
  names, the channel names a Core Audio or ASIO device gives; without them *Input 3*); inputs have a **live meter** (RMS bar, peak
  tick, *CLIP* held 1 s). The meters run before the session opens (the daemon opens the
  device capture-only, no output stream), so you can find a mic by tapping it.
- **Roles**: **R** *Reference* (the loopback return, one input); **M** *Mic*, a measurement mic (any
  number; **N** names one — the name is the mic's identity for calibrations; a named mic's
  row says its curve and calibration, and **←/→** choose the curve: off, 0°, 90° …);
  **S** *Stimulus* (the outputs the stimulus plays on; this app's choice, remembered per
  device; applied at once when the open session has the output). **Space** puts a row in or
  out of the session. The mouse works too: boxes, the *Reference* / *Mic* / *Stimulus* chips,
  double click on a name.
- **Output names**: **N** on an output names it for the whole rig (`Main L`; empty clears
  it); the top bar names the stimulus outputs by it (`→ Main L, Main R`).
- **Reference (loopback)**: names the reference pair in words (`Loop return (input 1) is the
  reference: it records what Main L (output 1) plays.`), with a line on what it is for.
- **Detect loopback…** (**D**): asks for a level (no default; your last stimulus level is
  offered), then on **Enter** plays a 0.5 s band-limited noise burst on the stimulus output —
  under the stimulus lease and the ceiling, faded — and marks the input it returns on as the
  Reference (or says none answered and which came closest).
- **Driver control panel…** (**P**, ASIO devices): opens the driver's own settings
  window (sample rate, buffer size, clock source, routing). On a Core Audio device the same
  key is **Audio MIDI Setup…**: it brings up macOS's Audio MIDI Setup, where the device's
  rate, format and clock source are set (for every app using it). A change there resets the
  driver; the open audio I/O reopens at the driver's new rate and buffer by itself, its
  measurements restart at them, and the top bar says what changed in the warning colour
  (`… · 44.1 kHz · 512 frames · driver changed from 48.0 kHz`, the hover has the
  details) until the session is next opened. The clock sources the driver offers are listed
  under the device, the one in use marked; choose among them in the driver's panel (Audio
  MIDI Setup on macOS).
- After the session line the top bar shows how the rig holds up:
  `CPU remote 23 % · local 41 % · DSP 14 % · 0 xruns · 14 Mbit/s · 30 fps`. **CPU** is the
  whole daemon host's use (every process, every core); with the daemon on another computer
  it reads **remote** and **local** is this one's, and with the daemon on this computer (or
  inside the app) a single `CPU 23 %`; **DSP** is the share of each buffer period ac2d's own
  audio callback takes (not the whole JACK graph's load); **xruns** count since the session
  opened; the data rate is what this window receives from the daemon (left out with the
  daemon inside the app); **fps** is shown while live data arrives. CPU past 85 %, DSP past
  70 %, a frame rate under a third of the data's and an xrun in the last 5 s show in the
  warning colour; the hover names the hosts (`remote = pupu (192.168.9.27)`,
  `local = ketunkolo`) and gives the means and peaks since the session opened. Each number keeps a
  fixed width, so the words after it stay put as its digits change. A figure not known yet
  is left out, never shown as zero. When the session line and these figures are wider than
  the room between the bar's left and right groups, they scroll slowly back and forth
  through it; resting the pointer on them holds them still.
- **Enter** opens; what is missing is said in words (*Pick a reference input: the loopback
  from your stimulus output*). Roles and mic names are remembered per device (`ui.toml`).
  On a daemon with no measurements yet, the session opening brings up the **New
  measurement** chooser (as **Ctrl+M** does): **Enter** on a kind opens its settings, **Esc**
  leaves the empty panes, which say how to come back to it.

The measurement dialogs pick inputs the same way: by name, with meters, **←/→**.

### Input meters

While a session is open, the top of the left-hand list shows **Inputs**, one meter per
captured input: bar = RMS, tick = sample peak, number = RMS dBFS, red *CLIP* held 1 s. They
run whether or not anything measures, sweeps included. Each row is named, not numbered:
the mic's name, else the backend's channel name, else *Input N*; then its role (*reference*
or *mic*); then the input number when the name lacks it — *MM1 34804 · 90° · mic (in 1)*,
*loopback · reference (in 2)*, *capture_4 (in 4)*. A named mic's row says which curve is in
use, or why none (*curve off*, *curve not chosen*, *no curve stored*, *90° not stored*). A
**REF** / **MEAS** mark shows what the running sweep, else the selected measurement, uses.

## Reference wiring and loopback

A transfer function divides what the microphone hears by what you sent. "What you sent" is
the **reference** input: the stimulus, taken electrically from the output that feeds the
system and looped back into an input of the same interface. Both inputs share one clock, so
the comparison is exact whatever the output side does.

```
out 1 ──┬──► processor / amp / speaker ··· mic ──► in 2   (measurement)
        └──► loopback cable ─────────────────────► in 1   (reference)
```

Without signal on the reference the transfer pane shows **NO REFERENCE**, with a detail:

| Situation | Detail shown |
|---|---|
| this app's stimulus off | `stimulus off: Space arms, Enter starts it` |
| armed and silent | `stimulus armed: Enter starts it` |
| on the sweep view (where Space arms a sweep) | `stimulus off: arm it from a transfer pane` |
| stimulus playing (this app's or another client's) | `reference silent: check the loopback cable` |

With a loopback that also returns the generator's own output (the **R** and **S** roles,
`session open --loopback-out 1 --loopback-in 1` from the CLI), the daemon continuously
checks output → input timing and warns about dropped or repeated output samples.

Any signal works, not only ac2's generator: programme material fed to the reference input
measures the same way (coherence then shows which frequencies the music covered).

## Stimulus

The generator plays pink, periodic pink, white noise or a sine, and is hard to start by
accident:

- nothing plays without a typed **level** (`L` in the app, `--level` in the CLI); there is
  no default;
- **arm** (`Space`) and **fire** (`Enter`) are separate steps; **Esc** stops (with a window
  open, Esc first closes it), **Shift+Esc** stops from anywhere, from any client. While
  anything is armed or playing the top bar shows **■ Stop: Shift+Esc**. Space pressed while
  a stop is finishing arms once it is done (Esc cancels that);
- one client holds the stimulus at a time (a refreshed lease); if it disappears the daemon
  fades the output out within 1.5 s;
- the rig's [system max level](#system-max-level) caps it, never above `ac2d --max-level`
  (default −10 dBFS RMS);
- loading a session or restarting the daemon comes up disarmed.

### What Space and Enter start

| Focused view | Space | Enter |
|---|---|---|
| transfer, spectrum / RTA (spectrograph included), IR, SPL | arms pink noise (or the chosen signal) at the operator's level on the stimulus outputs | plays it |
| sweep (the **Sweep / distortion** pane focused) | arms a **run of the selected sweep measurement** with its settings (outputs, level, length, range, repeats, silence after, reference); with no sweep measurement yet, opens the dialog that makes one | plays it |

A level or outputs changed while a sweep is armed become the sweep measurement's settings.
Armed and still silent, Space on a view of the other kind re-arms that view's stimulus. The
top bar says what the keys will do: `Space arms: sweep Genelec 1 m · 3 s −50 dBFS`,
`Enter fires: pink −50 dBFS → out 1`.

### Stopping the last transfer measurement stops the stimulus

The noise is there to excite transfer functions. When **S** (or the palette) stops, or a
delete removes, the last running transfer measurement, the app also stops the stimulus it
holds, armed or playing, as Esc does: `Main L stopped · stimulus stopped (no transfer
measurement left running)`. SPL meters, spectra and RTAs do not keep it playing; a sweep
and another client's stimulus are never stopped this way; `ac2 meas stop` stops only the
measurement (a script gets no side effect it did not ask for).

## Transfer measurement

The transfer pane shows **magnitude**, **phase** and **coherence** of measurement /
reference on a log-frequency grid (48 points per octave unless its
[resolution](#resolution-and-the-resolution-marker) says otherwise), computed with a
multi-time-window FFT ladder: long windows at low frequencies, responsive at high ones.
Each is its own view of the pane (**G** steps them).

- **Coherence (γ²)** is how much of the measured energy the reference explains. It is
  computed per FFT bin and averaged over each column, weighted by the reference's power in
  each bin, so it does not drop at a coarser resolution or where phase turns quickly. It is drawn as trace transparency, its own curve,
  or a mask blanking the traces where γ² is below a threshold (**B**: off, 0.3, 0.5, 0.7,
  0.9).
- **Averaging** accumulates the cross- and auto-spectra; **R** resets it. To keep a result,
  capture it (**Ctrl+1 … 9**) or stop the measurement (**S**).
- **Smoothing** (1/3 … 1/48 octave) is applied after coherence, so it never makes bad data
  look coherent.
- **Transfer stack** (palette *Transfer stack*): the focused transfer pane becomes three
  stacked panes of its measurement — magnitude on half the height, phase and coherence a
  quarter each (a focused pane of another kind becomes the stack of the selected transfer
  measurement). They are ordinary panes afterwards. The frequency axis is shared, so the
  three read down one column; the top pane carries the legend and NO DELAY ESTIMATE banner,
  fault banners (clipping, STALE, audio stopped) show in each.
- A **new transfer measurement** made in the app opens as a transfer stack in place of the
  focused transfer pane (else the transfer pane worked in last; with none on screen, split
  off the focused pane). Measurements from another client, the CLI or a restored session
  leave the panes alone.

### Panes: split, close, change what one shows

The app starts with one pane showing the kind of the daemon's first measurement (a transfer
pane on an empty daemon).

| Key | Does |
|---|---|
| **N** | splits the focused pane along its longer side; the new half shows the same measurement and views and takes the focus |
| **Shift+N** | turns the split holding the focused pane: side by side ↔ stacked; sizes and order stay |
| **Alt+→** / **Alt+←** | grows / shrinks the focused pane by a tenth of its split, down to the smallest readable size (the one **N** refuses to split below); dragging the gap between panes does the same |
| **Q** | closes the focused pane; its neighbour takes the space and focus. The last pane stays (**N** splits it, **Ctrl+M** makes a new measurement) |
| **Tab** / **Shift+Tab** | steps through the measurement list as a click on that row does ([below](#choosing-what-a-pane-shows)) |
| **G** / **Shift+G** | steps the focused pane's views forward / back (chains below) |
| **Alt+1 … Alt+9** | focus the panes in reading order: top to bottom, then left to right |

Each pane has its own kind, measurement and views; view keys act on the focused pane only.
Two transfer panes can show two measurements side by side, or one measurement as magnitude
and impulse response (**N**, then **G** the new half). The layout is remembered.

| Pane | G steps (Shift+G the other way) |
|---|---|
| Transfer | magnitude → phase → coherence → impulse response linear → log → ETC → magnitude |
| Sweep / distortion | response & distortion → impulse response linear → log → ETC → room parameters → response & distortion |
| Spectrum / RTA | spectrum → spectrum + spectrograph → spectrograph → spectrum |
| SPL | meter → Leq windows → meter + Leq → bands (with a band meter) → meter |

The transfer pane's **impulse response** views show the IR of that pane's measurement as
linear amplitude, dB re its peak, or energy-time curve (**Shift+G** from magnitude reaches
the ETC in one step). Coherence is always a view of its own; Settings › Display can also
draw it over the magnitude. A stopped measurement draws no live IR (its other
views keep the last curve, tagged `stopped`); a selected capture's stored IR still shows.

A command about a kind (an SPL view key, the sweep's views) goes to the pane of that kind
focused last; with none on screen the focused pane turns into one. A new sweep made with
**Shift+S** goes to the sweep pane focused last, else gets a pane of its own split off the
focused one. A finished sweep opens no pane: the toast says to **Tab** to it.

### Choosing what a pane shows

Each pane's title has a **chip** naming its measurement. Every pane draws that
measurement's **group** as the measurement tree lists it, and nothing else of its own
accord: its live curve (unless hidden), the math channels filed under it, and its shown
stored traces of the kinds the pane draws. A math channel chosen on the chip draws its
owner's group. A stopped measurement keeps its last curve, tagged `stopped` in the legend
and list; **S** starts it again. A pane never borrows another measurement's data: while its
own has nothing to draw (hidden, stopped before its first frame, or not measured yet) it
says so (`Spectrum 2: no frames yet`, `FOH SPL hidden — A shows it`).

| Pane | Draws |
|---|---|
| **Transfer** | the group's live transfer curve, math channels, captures, targets and sweep runs (a sweep measurement on the chip: its shown runs, no live curve) |
| **Spectrum / RTA** | the group's live spectrum or RTA, math channels and stored spectra / band sets; two spectrum measurements are two panes, each with its own curves |
| **Sweep / distortion** | one run of its sweep measurement: the selected run when it is one of that sweep's, else the run of it picked last, else its newest; never another sweep's run |
| **SPL** | its own meter only (no compare); to see two meters, add a pane and drag the other SPL measurement onto it |

**Imported** traces draw on no pane until moved under a measurement (select, **Move to
measurement…**, **Shift+F2**); while shown traces of the kind a pane draws wait under
Imported, that pane says so. A stored trace selected in the list changes no pane: drag its
row onto a pane, or pick its measurement on the chip, to see it there.

**Compare (C)** draws a curve of another group in a measurement's panes. Select a
measurement or stored trace and press **C**: *Compare Main R with…* lists the measurements
it can be drawn over, those on screen first (`· on screen`), a tick on the ones it is
already compared into. **Enter** ticks or unticks a row, **Esc** closes. Its own group is
not offered. From then on every pane showing that measurement draws it, tagged `cmp` in the
legend (its tree row too), and no other pane does. A transfer curve is compared into a
transfer measurement or a sweep, a spectrum or band set into a spectrum or RTA, a sweep run
into a transfer measurement or over another sweep's run (its fundamental and harmonics in
the run's own colour). Palette *Clear compare* empties the focused pane's measurement's
list. An Imported trace is not compared: **C** says to move it under a measurement first.
SPL meters are neither compared nor compared into: a meter is a number, read side by side.
Compare is display only: **M**, math operands and the phase reference use the group's own
curves.

Each measurement keeps its compare list on the daemon, in order, first compared first: every
app connected draws the same, a session saves it, and a deleted measurement or trace leaves
every list. A pane draws its own group on top, then the compared curves in that order, each a
layer further back: a fill or bar under every line, and a fill or bar drawn fainter the
further back it is (never below about half), so the pane's own curves stay readable.

To put another measurement in a pane:

- click the chip: every measurement in tree order, tagged with its kind, then the pane kinds
  no measurement needs yet. The pane turns into the kind picked; only it changes, and the
  selection in the list stays where it was;
- or drag a row of the measurement list onto the pane
  ([below](#dragging-a-row-onto-a-pane));
- or palette *Choose pane measurement…*.

A row of the measurement list clicked (or reached with **Tab** / **Shift+Tab**):

- a **measurement row** selects the measurement and shows it: its own panes when it has
  them ([Layouts follow the measurement](#layouts-follow-the-measurement)), else in the
  focused pane; panes no chip or drop has given a measurement show it too, when it is of
  their kind;
- a **child row** — a live curve, stored trace or math channel — only selects it, for the
  keys below; no pane changes. One no pane draws stays selected all the same.

Which measurement the keys act on:

- A click inside a pane focuses it; the selection stays on the row chosen in the list.
- Keys of a pane act on what that pane shows: **S** and **R** on the focused pane's
  measurement (they say what to create when it shows none), **X** and the delay keys on the
  focused transfer pane's measurement (on another pane: the selected one), **Space** on the
  sweep view on the focused sweep pane's sweep.
- Keys of the selection (**A**, **C**, **K**, **Delete**, the offset, colour, capture
  **Ctrl+1…9**, mic curve) act on the row selected in the list, whatever pane has the focus.

#### What A and Delete act on: the item selected last

One measurement and at most one stored trace are selected; the one selected last gets the
keys that act on "the selected curve". A click on a measurement row, live curve or math
channel (or **Tab** onto one) gives them to the measurement; a click on a trace (or **V**,
**Alt+V**) to the trace; **Esc** with no window open hands them back to
the measurement. The tree shows which: the measurement row is filled while it has the keys,
only outlined while a later-selected trace has them.

- **A** on a measurement hides its live curve in every pane of this app (the measurement
  keeps running; other clients still see it). Its list row says `hidden` (dimmed), its
  pane title starts `TF 2 hidden`, the IR view says `TF 2 hidden — A shows it`. Hidden
  measurements are remembered by name (`hidden = ["TF 2"]` under `[layout]` in `ui.toml`).
  While the app runs, hiding follows the measurement itself: renaming it keeps it hidden,
  a new one made under its old name starts shown, and of two measurements with the same
  name **A** hides only the selected one. On the next start a remembered name hides the
  one measurement of that name; a name two or more measurements share hides none of them
  and is forgotten (the name cannot say which was meant), so such a duplicate is not
  written to `ui.toml` either.
  A click on a legend row selects that curve, so click then **A** hides it.
- **Shift+A** hides a measurement and everything under it (live curve, traces, math
  channels), and shows them all when all are hidden (with a trace selected: its group).
- **Delete** / **Backspace** on a measurement that owns no stored traces asks: *Delete
  measurement TF 2? Its live curve and settings go; it owns no stored traces.* **Delete**
  or **Backspace** deletes (**Enter** does nothing there), **Esc** or **N** keeps. One that owns traces or math
  channels asks what happens to them
  ([below](#deleting-a-measurement-that-owns-traces)). A measurement a math channel computes from cannot be deleted; the
  window names the channel to edit or delete first.
- On a stored trace both act on the trace ([below](#rename-export-delete)).

#### One pane only, full screen (W)

**W** steps: split layout → the focused pane alone → that pane **full screen** → split.
Full screen is the **stage view**: no top bar, lists, pane titles or key hints; a plot keeps
a one-line caption naming the pane and measurement (the SPL meter and Leq windows name
themselves). Arming, playing, stopping the stimulus or a running sweep change nothing on it;
Esc and Shift+Esc stop as always.

**F** (or **F11**) steps the window: windowed → window full screen (top bar and lists as
before) → **panes only** (the stage view with every pane) → windowed. With one pane up (W),
the first **F** already gives the stage view and the next one returns to windowed. In panes only, **W** toggles all panes / the
focused one and stays full screen. The hidden lists still answer their keys (**Tab**,
**V**), a click focuses a pane.

With a pane maximised, a chip pick puts the measurement in that pane, turning it into the
kind that shows it (a sweep stays on a transfer pane, which draws sweeps too); the layout
stays maximised.

#### The layout comes back

`ui.toml` (written on layout change and exit) keeps the panes (split, kind, focus,
maximised / full screen / panes only), each pane's measurement by name, its views and **T**
step, the level axis ranges set on it by hand (`[layout.panes.levels]`), the Leq style and the sweep pane's
dB / %, the IR views' dB axes (`[ir_levels]`), and the window's size and position. The next start restores them without arming anything. A measurement
that is gone leaves its pane on its usual choice; one the daemon loses while the app runs
(restart, reopened or salvaged session) comes back to its pane and selection as soon as one
of that name exists again. A layout from an older version is dropped (one pane); a window
larger than the screen is shrunk to fit. On Wayland the system places the window.

#### Layouts follow the measurement

A measurement can carry its own panes. Choose its row (click, **Tab**), then edit the
panes — split, close, change a pane's kind or view, **T**, drag a split, lay out the
transfer stack, pick another measurement on a pane's chip or drop a row on it, zoom or pan
a pane's level axis — and the layout is stored on that measurement. Choosing its row again
brings its panes back; a measurement without a layout keeps the panes on screen. Only a
measurement row brings panes. A measurement picked into one of its panes is a guest there:
the edit is stored on the measurement whose row was chosen, never on the guest, and never on
any other measurement, even one whose panes happen to look the same. An edit after which no
pane shows that measurement is stored on nobody. The list says it: `· 3 own panes`.

The layout lives in the session with the measurement: saved and opened with it, and shown
by every app that selects it (another laptop, the kiosk), edits included. What travels is
the split, each pane's kind and view, the measurement each pane was picked for (a pick of a
measurement deleted since shows the layout's own measurement again) and the level ranges
set on it by hand; the focus, **W**, **F** and the window stay per app. A pane given no
measurement by a chip or a drop shows the measurement whose row was chosen. A level range
not set by hand is not kept: a spectrum pane frames its first curve when its panes come
back, as on first show; one set by hand stays until the pane shows another measurement.

`ac2 meas list` has a `layout` column; `ac2 meas layout clear <meas>` removes a
measurement's own panes. Deleting a measurement deletes its layout; editing it keeps it.

#### Dragging a row onto a pane

Any row of the measurement list (a measurement, its live curve, a math channel, a stored
trace) can be dragged onto a pane: hold the button and move a few pixels. A label beside
the pointer says where it lands (`FOH SPL → pane 2`) and the target is shaded faintly with
a dashed edge. Dropped in the middle of a pane, that pane shows the row as a pick from its
chip would (a live curve or math channel: that measurement; a stored trace: its
measurement, in the kind of pane that draws the trace), turning into the kind that draws
it; focus moves there and no other pane changes. The selection in the list stays. A drop is
a guest pick: it never brings the measurement's own panes, and the edit is stored on the
measurement whose row was chosen, as in *Layouts follow the measurement*.

Dropped within about the outer fifth of a pane (`FOH SPL → new pane below`), the pane splits
on that side as **N** would and the new half shows the row. A pane too small for **N** to
split takes a plain drop instead, and the label names the pane. **Shift** held on release
compares the row into the pane's measurement, as ticking it in **C**'s list does, and the
pane keeps what it shows (`Main R → compare in pane 2`); the row needs a curve of the kind
the pane draws, an SPL pane draws no compared meters, a row of the pane's own group is drawn
there already and an Imported trace is not compared, so anything else is refused with a
toast that says why. Two SPL meters side by side: add a pane (**N**) and
drop the other meter on it without **Shift**. **Esc** cancels the drag; a drop outside the panes does nothing.

#### Undo and redo (Ctrl+Z, Ctrl+Shift+Z)

**Ctrl+Z** takes back the last change to the panes and **Ctrl+Shift+Z** does it again; the
toast names it (`Undid: FOH SPL → pane 2`, `Redid: split pane 1`, `Nothing to undo`).
Covered: drops and edge drops, **N** / **Shift+N**, **Q**, chip picks, measurement rows
(click, **Tab** / **Shift+Tab**), **G** / **Shift+G**, **T**, a dragged split (one step per drag),
**Alt+←/→**, **W**, **F** and the transfer stack. Daemon actions (start, stop, delete,
capture, rename, delay, smoothing, compare lists, styles and offsets) are not undone. The
history keeps 50 steps per app instance, is not saved, and a new change drops whatever was
undone after it.

When the step undone or redone had been stored as the chosen measurement's own panes, the
restored panes are stored on it again; no other measurement's layout is written. A pane
whose measurement has since been deleted comes back empty, of its kind.

### Zoom, pan and the level axis

The frequency axis is shared by the panes: **I** / **O** zoom, **←** / **→** pan, **Home**
resets to 20 Hz – 20 kHz; the wheel zooms about the pointer, a drag pans.

Each pane's **level axis** (transfer magnitude, spectrum / RTA level, sweep distortion) is
its own: two transfer panes side by side zoom and pan apart, and the keys and the wheel move
only the pane they act on. A pane split off (**N**) starts on the range of the pane it came
from. A range set by hand (zoom, pan, **Shift+Home**, **Ctrl+Home**) is remembered with
its pane: in `ui.toml` once the panes are a layout of the operator's (the app's own first
pane, still taking the first measurement's kind, keeps its axis only while the app runs),
and with the chosen measurement's own panes in the session, so its row brings the range
back on any app; Settings → Display → *level axes* resets every pane's. A range the app
framed by itself is not remembered: a spectrum pane fits its level axis to the first frame
it shows, a spectrum that starts fits the spectrum panes' on its first frame, and a new
sweep result fits the sweep panes' (as **Shift+Home**, frequency left alone); a range set
by hand stays until the pane shows another measurement (or the next sweep result). The impulse-response views' axes are shared by kind:
one for the transfer panes' IR views, one for the sweep panes'.

- **Ctrl+I** / **Ctrl+O** zoom it about its middle; **Ctrl+wheel** about the pointer.
- **Ctrl+↑** / **Ctrl+↓** pan it by about a tenth of the range; **Shift+wheel** pans
  smoothly. Plain **↑/↓** stay the stimulus level.
- **Shift+Home** fits it to what the pane shows (offsets included; a few empty bins far
  below do not stretch it) and resets frequency to 20 Hz – 20 kHz — the way to look at very
  low signals. **Ctrl+Home** restores the defaults (transfer ±30 dB, spectrum 0 … −100 dBFS
  or 20 … 120 dB SPL, 20 Hz – 20 kHz).
- The spectrum pane keeps one range per scale (**dBFS** uncalibrated, **dB SPL**
  calibrated), so a calibration never leaves the curves off a dBFS-sized axis.

#### Grid, labels and cursor (T)

**T** steps the focused pane's plot: grid, labels and cursor → no grid or marks → traces
only (no grid, labels or cursor; for screenshots; the legend stays) → full. The marks are
the grey resolution marker and a multi-resolution spectrum's window changes (`171 ms`,
`85 ms` …): they go with the grid, and come back with it. A toast names the step
(`Transfer: no grid or marks`). Axis ranges and cursor keys keep working in every step. The step
belongs to the pane (kept when **Tab** changes its measurement, copied by a split) and
covers the spectrograph, the Leq history strip and the sweep's IR too. On the SPL pane the
step past full also takes the limit text: OK / NEAR / OVER, "stay ≤ … dB SPL" and "limit … dB SPL"
leave the Leq columns, tiles, meter + Leq and band bars, and the colours, limit lines and
the band view's allowed-level marks say it; on course, cooling down and not calibrated
stay. The 10 dB lines across the Leq columns and band bars go with the grid, and traces
only takes their scale's numbers too (`SPL: no grid or limit text`, then
`SPL: no grid, scale or limit text`, `SPL: grid, scale and limit text`). The bars, their
values, the band labels, the limit lines and the allowed-level marks with their key stay
at every step, and nothing moves: the scale keeps its room. The theme is in
Settings › Display and the palette (*Theme*).

#### Export a pane as PNG

Palette (Ctrl+K) **Export pane as PNG…** (no key) writes the focused pane's plot as an image
on the computer the app runs on, for a tuning report. It asks where, suggesting
`~/ac2-exports/<measurement>-<view>-<YYYYMMDD-HHMMSS>.png` (a name typed without an
extension gets `.png`; the folder is made). The picture is the plot as shown: the same
view, zoom, theme and **T** step, with its legend, axes, labels and cursor readouts, drawn
by the same renderer as the screen at twice the pane's on-screen pixels (sharper in print;
a very large pane is held to the GPU's texture limit). The pane's title strip (number,
name, measurement chip) and its key-hint line are window chrome and are not in it. A toast
names the file and its size (`Transfer pane exported to … (2400×1300 px)`). The command
line has no export: it has no renderer.

#### The transfer legend

The magnitude pane's legend lists every curve (name, colour, tags) on a plate that reads
over any number of curves; cursor values sit on a plate of their own on the side away from
the legend. **Drag** the plate anywhere; **drag its corner grip** to set how large it may
grow (longer rows are cut with `…`). By default it is at most 70 % of the pane tall; more
rows **scroll with the wheel** and its last line counts the rest (`3 above · 9 below`), so
every curve stays reachable by name. **Click** a row to select its curve (then **A** hides
it; a hidden curve leaves the legend, bring it back from the list). Palette: *Legend
top-left* / *top-right* / *bottom-left* / *bottom-right*, *Show/hide legend*. Position,
size and hidden state are kept in `ui.toml` (`[legend.transfer]`).

#### Impulse-response pictures

The transfer pane's IR views and the sweep pane's IR views share keys and mouse on a
**time axis** (ms re t = 0):

| Action | Time axis | Value axis |
|---|---|---|
| zoom | **I** / **O** and the wheel (about the pointer; the keys about the cursor while it is in view) | **Ctrl+I** / **Ctrl+O**, **Ctrl+wheel** |
| pan | **←** / **→**, a drag | **Ctrl+↑** / **Ctrl+↓**, **Shift+wheel** |
| show all | **Home**: the whole IR | **Shift+Home**: the whole IR, the value axis framing the curve (linear: ±110 % of its peak; log / ETC: from the noise to the peak) |
| defaults | **Ctrl+Home**: the whole IR, ±110 % of the peak, −60 … +3 dB | |

- The value axis is amplitude in FS (linear) or dB re the peak (log, ETC).
- Time zooms no closer than four samples and pans at most one IR length outside the IR.
- A click (or **Shift+T**, palette *Comparison cursor*) puts a time cursor; **Shift+←/→** step
  it (a sample when zoomed in, else 1/100 of the span); it reads `1.25 ms · +0.500 FS` or
  `1.25 ms · −12.3 dB`.
- Transfer and sweep IR views keep their own zoom and cursor; the log / ETC ranges are in
  `ui.toml` (`[ir_levels]`: `transfer`, `sweep`). A selected transfer trace's stored IR is drawn on the
  transfer IR view ([The impulse-response view](#the-impulse-response-view)).

The sweep's response & distortion view uses the frequency keys of the other panes, its
level axis in dB or %; its cursor reads the fundamental, each harmonic and THD.

#### What the spectrum's level means

A narrowband spectrum's levels are **per FFT bin**, and the axis says how wide a bin is:
`dBFS per 1.46 Hz bin (tone)` (48 kHz / 32 768 points).

- A sine reads its RMS level whatever the FFT length (*tone* level).
- Broadband sound spreads its power over many bins, so each bin reads far below the band or
  total level, 3 dB lower per doubling of FFT length. That is why a calibrated spectrum of a
  loud room can sit at 40 dB SPL.

> **Use an RTA for band levels in dB SPL** (its axis says `(band)`).

A spectrum's **Level** (New and *Edit measurement…* forms, `ac2 meas new spectrum --level
tone|density`, `ac2 meas set --level`) chooses:

- **tone** (`dBFS`, `dB SPL`): the per-bin level above. Default for one FFT.
- **density** (`dBFS/Hz`, `dB SPL/Hz`): bin power divided by its noise bandwidth. Noise and
  music read the same at any FFT length and on fixed or multi-resolution spectra (white
  flat, pink −3 dB/octave); a sine reads below its level by 10·log10 of the noise bandwidth
  (Hann at 48 kHz: 65 536 points 1.1 Hz → 0.4 dB; 4096 points 17.6 Hz → 12.4 dB). Default
  for multi-resolution. Axis `dBFS/Hz (density)`, row `· density`.

Captures keep their level (export kind `spectrum dBFS/Hz` / `spectrum dB SPL/Hz`). Averaging
or math across tone and density is refused: converting needs each column's noise bandwidth,
which an imported curve lacks. Switching between one FFT and multi-resolution takes the new
default level unless a level is given with it. The named width is the bin spacing; with
Hann, broadband sound reads 1.8 dB above that spacing alone (noise bandwidth 1.5 bins).
Curves on different FFT lengths share `per bin, mixed widths`; a narrow pane shortens the
unit.

#### The spectrum legend

It names every curve the pane draws (live spectra and RTAs, then shown captures) with its
colour and tags (`stopped`, `STALE 3.2 s`, `offset +3.0 dB`), in rows above the plot so it
never covers a curve; a narrow pane drops tags first, then shows `+2 more`. A click selects
the curve (**A** hides it).

#### How a live spectrum is drawn

A live narrowband spectrum is sent as display columns: one per bin while bins are wider
than 1/96 octave (up to about 100 Hz at 65 536 points, 48 kHz), above that one per 1/96
octave holding the highest bin. Tones keep their level and the cursor reads frequency to
within 1/96 octave, while the stream stays small enough for WiFi. A **capture** keeps every
bin: zoom into one for finer detail.

A long FFT updates every eighth of its window (about 6/s at 65 536 points; more overlap adds
work but no information); a short FFT steps by 1024 samples at 48 kHz (about 47/s; the step
scales with the rate), or by half its window when that is shorter. The curve **glides** to each new spectrum
over the time between them (30–200 ms); the cursor, peak hold and captures use the newest
spectrum itself, and stale, stopped or reconfigured measurements are drawn as they are.
Settings › Display › Spectrum "jumps to each new frame" turns gliding off (RTAs too).

**B** steps the curves through three styles: **line**, **fill** (the same line over a faint
area in the trace colour down to the bottom of the plot) and **bars**. Bars show band power,
so only RTA traces become bars; a narrowband spectrum in bars is drawn as the line, since a
one-bin tone as a bar would be thinner than a pixel. The fill follows the line exactly: a
tone stays a spike, a gap in the line is a gap in the fill, stale traces dim, and the peak
hold stays a line on top. **P** steps the peaks (below): the peak hold is the highest level
of each bin since it was turned on, drawn as a thin line over the curve.

The style, the peak mode and the spectrograph's history and colours belong to the
measurement, not the app or the pane: **B**, **P**, **Shift+B** and the colour maps change
the spectrum pane's measurement and say so (`Spectrum 1: fill`). The daemon keeps them, so
every connected app draws it alike and a session restores them; a curve compared into
another measurement's pane, and each stored trace, draws with its own measurement's style.
The pane's own view (**G**) and plot chrome (**T**) stay the pane's: they are about the space
on this screen, and two panes on one measurement may well want a spectrograph in one and the
plain spectrum in the other.

#### Peaks: hold and tracking

**P** (palette *Peaks*) steps the spectrum pane's measurement through **off → peak hold →
peak tracking → off** (`ac2 meas set <meas> --peak off|hold|track`), the toast naming the
step (`Sub: peaks: hold`). The mode is the measurement's; the hold and the tracked peaks
are each app's own, started when the mode asks for them, so a hold turned on again starts
afresh. Tracking draws no hold line: the boxes and the list say where the peaks are, and a
hold line under them would only crowd them. Tracking marks the five highest peaks of the pane's own live curve (the first live curve
when the pane's measurement has none) with a small box on the curve and lists them under
the cursor readout, loudest first: `1  1.00 kHz  −32.0 dBFS (band)`, the value's
quantity in parentheses as the cursor readout gives it. One curve is tracked, not
every curve shown: five boxes per curve on several curves crowd the plot and the list would
no longer read at a glance.

A peak is a maximum that stands at least 6 dB above the dips on both sides of it (each
dip the lowest point before the curve rises higher than the peak; an averaged noise floor
ripples by a few dB, so a mark always sits on something that stands out); peaks closer than 1/6 octave count as one, the
stronger. A narrowband peak's frequency is the vertex of the parabola through its bin and
the two beside it (within a fraction of a bin of a tone between bins); an RTA peak reads
its band's centre. The level is the curve's own at that column, what the cursor reads. Gaps,
invalid columns and a multi-resolution spectrum's unresolved columns hold no peak. Marks do
not hop: a marked peak keeps its place unless a rival beats it by more than 3 dB, and keeps
its row unless the one below beats it by more than 1 dB. **T** to traces only takes the list
and the rank numbers away; the boxes stay on the curve.

#### Fixed or multi-resolution

A spectrum's **FFT length** is one FFT (`64 pt` … `65536 pt`, default 65 536) or
**multi-resolution** (`multi-res 1/48 oct`; 1/12 … 1/96 oct). One FFT has one window for all
frequencies: 65 536 points resolve 0.73 Hz at 48 kHz but take 1.4 s to see a change; 2048
follow within 43 ms but cannot separate anything closer than 23 Hz. Multi-resolution runs
six FFTs, each half as long as the one before, and shows each frequency from the suited
window: at the low end as fine as 65 536 points at 96 kHz (1.46 Hz bins), at the top a
window of about 21 ms ([multires-spectrum.md](design/multires-spectrum.md)).

At 96 kHz (at 48 kHz the windows are the same length, half the points):

| Frequency | Windows | Updates every |
|---|---|---|
| below 203 Hz | 683 ms | 85 ms |
| 203 – 406 Hz | 683 + 341 ms | 85 / 43 ms |
| 406 – 812 Hz | 341 + 171 ms | 43 / 21 ms |
| 812 Hz – 1.6 kHz | 171 + 85 ms | 21 ms |
| 1.6 – 3.25 kHz | 85 + 43 ms | 21 ms |
| 3.25 – 6.5 kHz | 43 + 21 ms | 21 / 11 ms |
| above 6.5 kHz | 21 ms | 11 ms |

Between two windows' frequencies a column blends both, so a level never steps.

- **Level**: density by default (`dBFS/Hz (density, multi-res)`): on white noise it agrees
  with a 65 536-point spectrum within 0.15 dB rms over every column. **Tone**
  (`dBFS (tone, multi-res)`) reads a sine at its RMS level, but broadband sound then reads
  in a bandwidth that grows with frequency (about 1/64 octave from 203 Hz to 6.5 kHz):
  pink noise reads flat, white rises 3 dB/octave, and pink reads about **15 dB higher** at
  3 kHz than on a 65 536-point FFT at 48 kHz though nothing changed but the analyzer.
- **Window edges**: a faint dashed line marks each window's frequency, labelled with its
  length, on the spectrum and the spectrograph; the cursor names the windows behind its
  column (`1.00 kHz · 171 + 85 ms windows`). The low columns change only when their long
  window has a new frame. Columns below what the longest window resolves at the chosen grid
  (below about 101 Hz at 1/48) are shaded as by the
  [resolution marker](#resolution-and-the-resolution-marker), whose toggle hides both.
- **Averaging** counts frames of every window alike: `FIFO 16 frames` averages the top over
  16 × 11 ms, the bottom over 16 × 85 ms. At most 128 frames.
- **Captures** keep the multi-resolution columns.
- Cost: about 4× one 65 536-point FFT — about 2 % of one core at 96 kHz on a current
  desktop, an estimated 5–7 % on a 2011 laptop (Core i5-2415M).

#### Averaging (spectrum and RTA)

Spectra and RTAs start **unaveraged**: each result is one frame, so fluctuating sound jumps
by several dB. Averaging is always on **power**, never dB (0 dB and −20 dB average to −3 dB,
what an integrating meter reads).

| Averaging | Reads | CLI (`--average`) |
|---|---|---|
| `off` | each frame as it comes | `off` |
| `exp τ 2 s` | exponential: older sound fades with time constant τ; until τ has passed, the plain power mean so far | `exp:2s` |
| `FIFO 64 frames` | the mean of the last 64 frames, each counted equally in time | `fifo:64` |

A spectrum's frame is one hop of its FFT (0.17 s at 65 536 points, 48 kHz; 1024 samples for
short ones, half the window for the shortest), so a FIFO's span depends on FFT length; an RTA's frame is one result interval,
about 1/60 s. A time constant means the same for both, so the dialogs list those first. A
spectrum FIFO holds at most 511 frames at 65 536 points; an RTA's at most 65 536.

- **New spectrum** / **New RTA** have an *Averaging* field (**←/→**); *Edit measurement…*
  changes it, the input, name and smoothing or bands, and keeps the rest. A CLI setting the
  list does not offer shows as it is.
- A spectrum's dialogs also pick *FFT length* (64 … 65 536 points, or multi-resolution
  1/12 … 1/96 oct) and *Window* (Hann, Blackman-Harris, flat-top, rectangular); the hint
  says what it gives at the session's rate (`0.73 Hz bins · 1365 ms window · 5.9 updates/s`;
  `0.68 s window at LF … 21 ms at HF`). A new resolution or window restarts a running
  spectrum. A spectrum named in a math channel keeps its resolution (operands share one bin
  grid); its window changes freely.
- The list says it: `RTA  Room · running · avg FIFO 2400 frames`; so does `ac2 meas list`.
- **R** restarts the average; so does changing it.

```sh
ac2 meas new rta --input 2 --name room --average exp:2s
ac2 meas new spectrum --input 2 --name fft --average fifo:16
ac2 meas set room --average fifo:2400     # about 40 s of RTA results, a plain power mean
ac2 meas set main-l --blocks 16           # a transfer measurement's averaging
ac2 meas set fft --fft 8192samples --window flattop
ac2 meas new spectrum --input 2 --name live --resolution multi      # multi-res 1/48 oct
ac2 meas set live --resolution multi:1/96
ac2 meas set live --fft 65536samples                                # back to one FFT
```

### Spectrograph

**G** in the spectrum pane steps spectrum → spectrum + **spectrograph** → spectrograph
alone ([spectrograph.md](design/spectrograph.md)). Frequency runs across on the spectrum's
axis, time down with the newest at the top, level as colour (viridis by default). The colour
bar spans the pane's level axis, so the level-axis keys change the colours.

- The palette (`spectrograph colours`, `magma`, …) picks the colour map of the pane's
  measurement: viridis, magma, inferno, cividis or grey. Magma, inferno and grey stay dark
  for most of their range, easier on the eyes in a dark room; all five are perceptual
  (brighter is always louder) and colour-blind safe. The measurement keeps it (`ac2 meas set
  <meas> --colours magma`), and the colour bar follows it.
- **Shift+B** steps the measurement's history 10, 30 (default), 60, 120 s (`ac2 meas set
  <meas> --span 60`); every length keeps a row per 33 ms. It starts afresh at each length
  and when the spectrograph comes into view.
- A click puts the cursor: `1.00 kHz · 4.2 s ago · −32.0 dBFS`.
- Time without frames (STALE, stopped) is a gap, never the last spectrum stretched. Where a
  pixel covers several frames or frequencies it shows the highest level, so short events
  and narrow tones are never lost.
- A stopped measurement keeps its spectrograph (`stopped`); changing FFT length, band
  fraction or calibration starts it over. The app builds it from the frames it already
  receives; nothing is kept while it is hidden.

**Two spectrographs side by side**: with the spectrum pane focused click `Spectrum 1`'s
row; **N**; on the new half pick `Spectrum 2` from its chip (or click its live-curve row);
**G G** on each half. Each half keeps its measurement and view.

### Resolution and the resolution marker

**Resolution** is how many points per octave a transfer or sweep curve has: **1/12, 1/24,
1/48 (default) or 1/96 oct**. It is the grid the curve is computed on, not smoothing.

- **Setting it**: *Resolution* in the New / Edit dialogs, `ac2 meas new tf|sweep …
  --resolution 1/96`, `ac2 meas set <meas> --resolution 1/24` (`ac2 ir capture
  --resolution`). The list shows a non-default one (`resolution 1/96 oct`). A transfer restarts its
  averaging on change; a sweep uses it from its next run.

Every estimate has a **resolution cell**, 1/T for a window of T seconds. Where it is wider
than a grid column, neighbouring columns share one estimate. The **resolution marker**
shows where: a faint grey shade with dashed edges, and the cursor says it
(`52.0 Hz · coarser than 1/96 oct (gaps)`).

- **Transfer**: the FFT ladder's windows are fixed. At 96 kHz: 1/48 is marked only below
  about 68 Hz; 1/96 below about 135 Hz, about 200–410 Hz and 1.6–3.3 kHz (at 48 kHz the last
  is 810 Hz–1.6 kHz); 1/24 only about 31–34 Hz; 1/12 nowhere. A live curve shows **gaps**
  where columns have no estimate of their own.
- **Sweep**: the window is the gate (or the response up to the noise); its edge is about
  69/W Hz at 1/48 (138/W at 1/96) for W seconds: an ungated room response marks at most the
  lowest columns, a 10 ms gate everything below about 7 kHz. The curve is **interpolated**
  there. Harmonic order k read in a window of W has cells 1/(k·W) wide on the
  fundamental's axis, so the distortion pane's edge sits higher;
  [LF harmonics: fine](#low-frequency-harmonics-lf-harmonics-fine) moves it down (to less
  than half). Distortion is drawn 1/24-octave band-averaged; the marker there refers to the
  harmonic estimates.
- Stored transfer captures and math channels carry no marker; sweep runs do.
- Settings › Display › Resolution marker turns it off (on by default).

### Smoothing

**K** coarser (off → 1/48 → 1/24 → 1/12 → 1/6 → 1/3 octave), **Shift+K** finer, palette
*Smoothing 1/6 oct* … sets a step. Transfer and spectrum panes; the title and each
transfer legend row say the smoothing.

- **Transfer functions** smooth the magnitude (power) **and the phase**, so phase, unwrapped
  phase and group delay read the smoothed curve. Phase is unwrapped within each run of valid
  columns before averaging, so a wrap never drags it toward 0°. That needs the delay set: a
  residual delay of more than about 34 periods (3.4 ms at 10 kHz) turns the phase more than
  half a turn between columns, beyond unwrapping — set the delay (**X**, **D**) first.
  `ac2 meas new tf … --smooth 6 --smooth-magnitude-only` smooths magnitude only (`mag only`).
- **Shift+P** steps the phase view: **wrapped → unwrapped → group delay → wrapped**.
- **Group delay** is the slope of a coherence-weighted line fit to the
  unwrapped phase over a span around each column, not a two-neighbour difference: on a
  48/octave grid neighbours are 3 % apart, so hundredths of a degree of phase error would
  swing LF group delay by tens of percent. The span is 1/12 octave or the phase smoothing
  when wider; the title says it (`Group delay ms · 1/12 oct`).
- **Spectra** are smoothed as power over a fractional-octave window on the bins; a smoothed
  spectrum no longer reads a sine's level, so the axis says `(tone, 1/6 oct smoothed)`.
  Spectra start unsmoothed (`ac2 meas new spectrum --smooth 6`).
- **RTA** bands already are fractional-octave: **K** says so and changes nothing.
- On a **live measurement** the keys act on the focused pane's measurement; nothing
  restarts.
- On a **stored trace**: select it (**V** / **Shift+V**, or click it), then **K**. The pane
  title names it (`Sweep 2: smoothing off`). Back to live: **V** past the last trace, click
  the trace again, palette *Deselect the stored trace*, or select a measurement.
- CLI: `ac2 trace smooth <trace> 1/12` (`1/3` … `1/48`, `12`, `none`); a transfer or sweep
  trace keeps its mode unless `--phase` or `--magnitude-only`.

Smoothing never changes stored data: captures keep the unsmoothed curve (starting with the
measurement's smoothing); averages and math channels combine unsmoothed curves.

#### Banners

Banners say what is wrong rather than show a misleading curve: **NO REFERENCE**, **NO
SIGNAL**, **CHECK ROUTING**, **CLIP**, **STALE** (with the age: `STALE · 3.2 s`), **NO DELAY
ESTIMATE**, **CLOCK DRIFT** (in ppm), **OUTPUT TIMING JUMP**, **AUDIO STOPPED** (with the
reason), **DAEMON NOT RESPONDING**, and for math channels **AVERAGE · 3 OF 4 POSITIONS** / **NO AVERAGE** / **NO RESULT**.

### Min / max tracking on the transfer function

**P** on a transfer pane turns min / max tracking on or off. It marks the five highest
peaks (▲) and the five deepest dips (▼) of one curve's magnitude (the selected curve, else
the pane's own) and lists them on a plate in a bottom corner of the magnitude view, clear
of the legend and the cursor values: `max` and `min`, each row rank, frequency and dB. It
reads the magnitude as drawn: smoothed, with its offset, without coherence-blanked or
invalid columns. Peaks and dips follow the spectrum's rules (6 dB prominence, 1/6 octave
merge, marks that do not hop; see [Peaks: hold and tracking](#peaks-hold-and-tracking)).
When four or more neighbouring dips are evenly spaced on a linear axis (within 10 %), as the
notches of a comb are (a copy of the sound arriving `τ` later cancels every `1/τ` Hz), the
list ends with the spacing and the delay it implies: `comb Δf 1.00 kHz · 1.00 ms`. An
uneven set shows nothing. **T** to traces only takes the list away and keeps the marks.

### Math channels

A **math channel** is a live result made from other curves by name: **A ÷ B**, **A × B**,
**A + B**, **A − B**, or the **average** of several ([math-channels.md](design/math-channels.md)).
Operands are live measurements or stored traces (captures, sweeps, imports) of one kind; it
is drawn where its kind is drawn. The daemon computes it, so it follows live operands and
every client (and `ac2 meas list`) sees it.

**Shift+M** (palette *New math channel…*): **A** (←/→ through every live measurement and
stored trace, `(live)` or `(stored, S2)`), the **operator**, **B** (curves of A's kind). The
name follows the expression (`Main L ÷ Sub`) until you type one. **Enter** creates and
starts it. *Edit measurement…* on a selected math channel reopens the dialog.

Transfer functions combine as complex values (magnitude and phase):

- **A ÷ B** — A relative to B (a previous measurement, or a target curve, magnitude only);
  its phase keeps A's arrival relative to B.
- **A × B** — the cascade: a response through a filter.
- **A + B** — the summation prediction (e.g. main + sub before both play). Relative arrival
  matters, so both must share a time base: live measurements and captures of the same audio
  session. The phase is referred to A's delay (or the one you pick).
- **A − B** — the complex difference.

Spectra and RTA bands are levels: **A − B** is the dB difference, **A + B** the power sum,
the average the power mean. **A × B** with B a stored transfer function (an imported IR, a
capture) is A heard **through** B: each column gains B's magnitude (an RTA band the power
mean of B over the band); **A ÷ B** takes it back out. A spectrum combines with a spectrum
of the same FFT length, an RTA with one of the same bands; the daemon says why it refuses
anything else.

**What a speaker does to a measurement:** import its impulse response
([Impulse responses as WAV files](#impulse-responses-as-wav-files)), then **Shift+M**: A the
live transfer (or spectrum / RTA), **×**, B the import. The transfer result's `delay` is the
sum of both, tagged `phase: own alignments` (the file has its own time base).

| Legend | Means |
|---|---|
| `Main L ÷ Sub` | the expression |
| `Main L + Sub · no coherence` | a sum has no coherence of its own |
| `phase: own alignments` | an operand shares no time base with the other (an import, a capture from an earlier session): their relative arrival is unknown |
| `magnitude only` | against a target |
| `Main L ÷ Sub · arrival Δ +3.7 µs · +1.3 mm @ 20 °C` | ÷, − and + of two transfer functions on one time base: how far apart the operands arrive (to 0.1 µs) |

**A ÷ B** and **A × B** take the lower coherence of the two per frequency, so the mask blanks
where either is unreliable.

#### The average (spatial average)

A speaker sounds different from seat to seat, so tune to the average of several mic
positions ([spatial-average.md](design/spatial-average.md)). Make one transfer measurement
per mic (same reference), then **Shift+M**, operator *average of several*: every curve of
A's kind is listed and included (**←/→** leaves one out), with a method and smoothing. The
legend counts: `Average of 4 · 4 positions · power avg`.

- **power** (default): the level over the positions without cancellation; EQ against this.
- **complex**: what one point summing the arrivals would hear (cancels where arrivals
  differ).
- **coherence-weighted**: the complex mean, cleaner positions counting more.

One mic moved from seat to seat: capture each position (**Ctrl+1 … 9**) and average the
captures the same way, or with **M**.

#### Missing operands

An operand that is stopped, settling, CLIP, NO REFERENCE, CHECK ROUTING, NO SIGNAL or
incompatible is left out and named (**AVERAGE · 3 OF 4 POSITIONS**; legend `3 of 4
positions`). One curve is never passed off as the result: fewer than two positions is
**NO AVERAGE**, a missing operand of a ratio or sum **NO RESULT · 1 OF 2 OPERANDS**. An
operand cannot be deleted, or moved to another grid, while a math channel names it. **R**
resets a math channel's live operands; **Ctrl+1 … 9** captures it, naming the expression and
operands.

```
ac2 math new "Main L / Sub"                       # ÷; also * + - (spaced), or ÷ × −
ac2 math new --name Prediction --op add --a "trace:Sub alone" --b "Main L"
ac2 math new --name Audience --op avg --of "Seat 1,Seat 2,Seat 3" \
    [--method power|complex|coherence] [--phase-ref "Seat 2" | --ref-delay 12ms] [--smooth 6]
ac2 math set Audience --method complex            # what is not given stays
ac2 math set Audience --under "FOH"               # listed under another measurement
```

A math channel is filed under `--under MEAS` (or `--imported`); without it under its first
operand when that is a measurement, else where that trace is filed (the app: the
measurement selected when Shift+M was pressed). Operands are names or ids; `trace:NAME` /
`meas:NAME` when a measurement and trace share a name.

#### Alignment: delay and polarity of two systems

Two systems that overlap in a band — sub and main, main and front fill, main and a delay
tower — add fully there only when they arrive in phase
([alignment.md](design/alignment.md)). **Alt+M** (palette *New alignment…*) is the math
dialog with the operator **↔ aligned**: **A** and **B** are any two transfer traces, live
`(live)` or stored (captures, sweeps, imports, two under one measurement too), on one time
base; **Overlap from / to** (Hz) is the band to fit; **Max delay** (default 50 ms) the
longest delay looked at. The band is never assumed: it is prefilled with where both curves
are alike (within 6 dB of each other and 20 dB of their own peak, *where both curves are
alike: check it*), or left empty to type. **Enter** creates it and opens its own pane, split
from the focused one, on the **alignment view**.

The daemon fits the delay over the band, weighting each frequency by `|A|·|B|` and the lower
coherence (where one system is far below the other nothing can cancel), and tries both
polarities. The view (**G** on a transfer pane reaches it only when its group has an
alignment) shows the phase difference before (faint) and after, the summation before and
after (0 dB: they add fully), the band shaded, and the readout:

```
Main: delay 4.21 ms · 1.44 m @ 20 °C · polarity normal
residual ±12° over 60–120 Hz · summation −7.9 dB → −0.3 dB (worst −21.0 dB → −1.2 dB)
other polarity: Main +9.80 ms inverted, ±41°
also fits: Main +15.30 ms, ±14° — the band is too narrow to tell: pick by distance
```

The last line appears when a delay one period away fits almost as well: a narrow band cannot
tell them apart, the distance between the boxes can. A live operand keeps the result live.
The curve of the alignment itself is the aligned sum (legend `Sub ↔ Main aligned · Main
+4.21 ms`), drawn and captured like any sum. Without three usable columns in the band it
reads **NO FIT** with the reason. ac2 applies no delay itself: dial it into the DSP and
measure again.

```
ac2 align new "Sub" "trace:Main alone" --band 60..120 [--max-delay 50ms] [--temp 20c] [--json]
ac2 align show "Sub ↔ Main alone" [--temp 18c] [--json]
```

## Delay finder

Phase is readable only after the propagation delay from speaker to mic is compensated. The
delay finder estimates it from the impulse response, with a **confidence** and the
**candidates** it considered.

- **X** finds and inserts the **first arrival** (the direct sound, what alignment needs);
  **Shift+X** the **strongest** peak.
- An ambiguous result (two similar arrivals, a strong reflection) lists the candidates:
  **1–3** (or ↑/↓ and Enter) pick one. With no usable estimate nothing is inserted.
- **Arrivals merged into one peak** (a single candidate): two arrivals closer than the band
  can separate, or a crossover inside the band (a multi-way box in the full band). **1**
  inserts the peak's delay; for the first arrival, run the finder in a band without the
  crossover (palette *Delay finder: mid / sub band* or a custom band, CLI `--band`).
- The band is **auto** unless you pick one: full (2–16 kHz), then mid (300 Hz – 3 kHz), then
  sub (20–120 Hz), the first the finder does not refuse. Palette *Delay finder: auto / full /
  mid / sub band*, *custom band (Hz)…*, *observation length (s)…*; CLI `--band
  full|mid|sub|auto|<lo>-<hi>` and `--observation`.

#### Setting the delay by hand

A measurement has one delay, the daemon's.

| Key | Does |
|---|---|
| **D** | types it in ms (`12.5` or `12.5ms`); −10000 … 10000 ms, negative = the measurement leads. Samples (`600samples`) or a distance (`4.3m`, at the set temperature's speed of sound) go through the CLI ([below](#from-the-cli)) |
| **,** / **.** | step it by 0.1 ms |
| **Ctrl+,** / **Ctrl+.** | step it by one sample |
| **Alt+,** / **Alt+.** | step it by a tenth of a sample |
| **Y** | tracks it continuously |

**The arrival stays; the live curve moves.** Only the live curve moves, at once; stored
traces stay (**,** / **.** on a selected stored trace move that trace). The list says
`delay 12.60 ms (+0.10 ms from arrival)`, the legend tags `+0.10 ms from arrival`, the toast
`Main L: delay +0.1 ms → 12.60 ms (+0.10 ms from arrival)`. The reference line and distance
keep the measured arrival. A capture taken then is drawn where the live curve was. Averages
are turned to the new delay rather than restarted, so you can walk the phase into place by
eye.

- **D** does the same move in one go (`Main L: delay 12.00 ms (−0.50 ms from arrival)`).
- The finder's **1** sets a new arrival: nothing moves and the offset is gone.
- Tracking (**Y**) follows the arrival and keeps your offset on top.
- A stopped or hidden measurement has no live curve: the keys say so and change nothing.

**Fractions of a sample.** 0.1 ms is fractional at most rates (9.6 samples at 96 kHz); the
daemon holds the delay to a millionth of a sample, so stepping out and back lands exactly
where you started, and the list shows delays to the microsecond. Tracking moves to a
fractional arrival once two windows agree within 0.1 sample, and leaves the delay alone
while the arrival stays within 0.05 sample.

**A larger change keeps what it can:** each analysis stage keeps its averages while the
change is small next to its window (about 2.4 ms at full rate, 9 ms and 28 ms for the lower
ranges at 48 kHz); only stages beyond that restart and show *settling*.

#### From the CLI

```sh
ac2 delay find main-l --insert
ac2 delay find sub --band 40hz-120hz        # limit the band
ac2 delay nudge main-l -0.25samples         # move it by a step
ac2 delay set main-l 600.25samples          # move it to a value as D does (the arrival stays)
```

#### The impulse-response view

The inserted delay is the time origin of the transfer pane's IR view. It carries the
measurement's banners (NO REFERENCE, NO SIGNAL, AUDIO STOPPED, DAEMON NOT RESPONDING); a
kept IR is tagged and dimmed like its transfer curve. A stopped measurement's last IR is not
drawn (it is no longer the system's). Without an IR it says why: `Main L stopped — S starts
it`, `no reference: nothing is playing — Space arms, Enter starts the stimulus` (or
`armed — …`, or `nothing is driving the loopback`), `no signal: …`.

**A stored IR.** A transfer capture keeps the IR shown at that moment (every sample, t = 0
at the trace's inserted delay), unless there was none (no reference). **Select** the trace
and the IR view draws its IR in the trace's colour, labelled as its legend row:
`t = 0 at inserted delay 12.50 ms · 4.29 m @ 20 °C · pre-EQ`. It goes back to the live IR
when the trace is deselected or has no IR (an average, a math capture, a text import). Only
a trace of the pane's group, or one compared (**C**), is picked up. The IR travels with the
trace in sessions, autosave and ac2 CSV exports.

## The measurement tree

Beside the panes, one tree lists every measurement with what it owns
([measurement-tree.md](design/measurement-tree.md)):

```
TF  Main L            running
  ├ Main L (live)
  ├ pre-EQ            capture
  ├ post-EQ           capture
  └ pre ÷ post        math (live)
SWEEP  Genelec 1 m    2 runs
  ├ Run 1             sweep run · 3 s −50.0 dBFS
  └ Run 2             sweep run · 3 s −50.0 dBFS
SPL  FOH SPL          running
Imported              1 trace
  └ 1083 94cm         imported
```

| Item | Filed under |
|---|---|
| **capture** (Ctrl+1 … 9) | the measurement it came from |
| **math channel** (Shift+M) | the measurement selected when it was made; its captures under the same measurement |
| **sweep run** | its sweep measurement |
| **average** (M) | with its inputs, when they share one |
| **imports**, session traces of no measurement | **Imported**, listed last when it holds something |

- The arrow before a measurement **folds** it (palette *Fold / unfold the selected
  measurement in the list*).
- A row's dot is its curve's colour (a ring when hidden): a click on it shows or hides the
  curve (a live curve: this app's display; a stored trace: the daemon's). A click on the row
  selects it: a measurement row also brings its own panes, a child row (live curve, stored
  trace, math channel) only selects and no pane changes
  ([Choosing what a pane shows](#choosing-what-a-pane-shows)); a click on the selected trace
  deselects it. A double click on a trace renames it.
- **Tab** / **Shift+Tab** select the next / previous listed row, wrapping (hidden rows too;
  the Imported header and folded rows skipped), as a click on it does. A selected live-curve
  row is marked and **A** hides that curve.
- **Move to measurement…** (**Shift+F2**) files the selected stored trace or math channel
  under another measurement or Imported (**↑/↓**, **Enter**, **Esc**). Only where it is
  listed changes (a locked trace moves too).
- **Colour families**: each measurement's live curve has the family's colour, its captures,
  runs and math results lighter and darker shades of the same hue, alternating between the
  far ends of the range so consecutive curves are easy to tell apart. Imports take colours
  of their own in list order (unused hues first, then shades, grey last); a moved trace takes
  its new measurement's colours. Deleting a measurement leaves the others' colours alone.
- **Shift+C** steps the selected measurement to the next colour no other measurement has
  (grey last); palette *Previous colour* steps back. The daemon keeps it (sessions,
  autosave), so every app shows it. `ac2 meas set <meas> --colour 1…8|grey|auto`. A math
  channel takes its measurement's colours.

### Deleting a measurement that owns traces

It asks every time: *Keep them (move to Imported)*, *Delete them too*, or *Cancel*
(**←/→**, **Enter**, **Esc**). An answer that would leave a math channel without an operand
cannot be taken (Keep with a math channel under it computing from the measurement; Delete
when a math channel elsewhere uses one of its traces). **Enter** keeps them by default, or
cancels when Keep cannot be taken. A measurement owning nothing
gets the plain confirmation. CLI: `ac2 meas rm` needs `--keep-traces` or `--delete-traces`.

## Traces and slots

A **trace** is a stored snapshot of a live result with the metadata to interpret it later:
delay, polarity, offset, smoothing, calibration state, mic and time. Its curve is stored
unsmoothed.

### Capturing and selecting

- **Ctrl+1 … Ctrl+9** capture the selected measurement into slot 1–9 (replacing what was
  there); **1 … 9** show and hide a slot.
- The tree lists every trace under its owner: name, kind (*capture*, *sweep run*,
  *imported*, *average*, *A ÷ B*, *target* …), slot, *hidden*, colour dot. Within a
  measurement slotted traces come first by slot, then the rest oldest first. A click selects
  (again: deselects).
- **V** / **Shift+V** select the next / previous **shown** trace in tree order, with the live
  measurement between last and first; **Alt+V** / **Alt+Shift+V** include hidden ones. Like
  a click on a child row, **V** only selects: no pane changes, and a trace no pane draws
  stays selected all the same.
- **End** (palette *Select live*) goes back to the live measurement. **Esc** only stops the
  stimulus: the stop reflex never changes the selection.
- **A** shows or hides the selected trace.
- Palette *Move the selected trace to slot…* puts it in slot 1–9 (`none` frees its slot).

### Trace keys

On the selected trace when its curve is on the transfer pane (else the live measurement):

| Key | Does |
|---|---|
| **U** | inverts it |
| **,** / **.** | move its delay by 0.1 ms (`Sweep 2: delay +0.1 ms → +0.30 ms from arrival`) |
| **E** | makes it the phase reference |
| **K** / **Shift+K** | smooth it |
| palette *Mic curve on the selected trace…* | corrects it ([below](#mic-curve-on-a-stored-trace)) |

A stored trace keeps only its offset from the arrival it was measured at, so no absolute
delay is shown. A target curve takes an offset only; a locked trace refuses. The pane title
names the selected trace and the plot marks it (a bar and thicker swatch on its legend row,
a line twice as wide).

### Spreading curves apart (display offset)

| Key | Moves the selected curve |
|---|---|
| **Alt+↑** / **Alt+↓** | up / down by 1 dB |
| **Alt+Shift+↑** / **Alt+Shift+↓** | up / down by 3 dB |
| **Alt+Home** | back to 0 dB |
| **J** | to a typed value (transfer and spectrum panes) |

The selected curve is the selected stored trace (any kind), else a live curve compared
into the focused pane once picked there (legend or list), else the focused pane's
measurement. A trace's offset is part of its record (`ac2 trace list`, sessions, exports); a
live measurement's is kept with the measurement on the daemon (`ac2 meas set <meas> --offset
6`, sessions), levels as measured: the curve keeps it wherever it is drawn, compared into
another measurement's pane too. The plot says it beside the curve
(`Main L S2 · Δt 0.00 ms · +3.0 dB`, `Main L S2 · offset +3.0 dB`), so a spread is never read
as a level difference. A locked trace keeps its offset.

### Rename, export, delete

- **F2** (palette *Rename trace…*, or a double click) renames the selected
  trace; `ac2 trace rename <trace> <name>`.
- Palette *Export trace…* writes it as `ac2 trace
  export --csv` does: a file path, or a folder (named after the trace, `Main L S1.csv`); a
  path ending in `.wav` writes its impulse response. The prompt starts in the last export's
  folder (first: home); the file is written on this computer, also with a remote daemon.
- **Delete** / **Backspace** asks, naming the trace; **Delete** or **Backspace** deletes
  (**Enter** does nothing there: it is the stimulus's key), **N** or **Esc** keeps. The selection moves to the next shown trace (else the one
  before, else live). A locked trace is not deleted. With a measurement selected after the
  trace, the keys are about the measurement.

### Comparing traces

- A run selected in the list or with **V** is the one the **Sweep / distortion** pane of
  its sweep measurement shows; **V** / **Shift+V** on that pane step through that sweep's
  runs and select them. A finished sweep is selected. **C** on another run draws it over
  the pane's run, tagged `cmp`.
- Overlays are drawn relative to the selected trace's measured delay, so relative arrival
  between traces stays visible.
- A click (or **Shift+T**, palette *Comparison cursor*) puts the comparison cursor, synchronised
  across panes; **Shift+←/→** move it.
- **C** compares the selected trace or measurement into the measurement you choose; its
  panes draw it whatever its group (not an Imported trace: move it under a measurement
  first).
- **M** averages the shown stored traces of the transfer pane's group (power; complex and
  coherence-weighted in the palette). For ÷ × + −, **Shift+M** with the trace selected
  starts with it as A.
- **Z** loads a target curve; the palette imports CSV and other analyzers' text exports.
- `ac2 trace display <trace> on|off`, `ac2 trace slot <trace> 3` (`none` frees it); `ac2
  trace list` shows both.

### Export and import

`ac2 trace export <trace> --csv out.csv` (`-` for stdout) writes the ac2 CSV: a `#` header
with every metadata field, then one row per column. The columns are always **as measured**:
offset, polarity, nudge, smoothing and an after-capture mic curve are listed in the header
but not applied, so an export re-imports exactly. `ac2 trace import out.csv` restores its
name, kind and **delay** (`# delay_ms:`); other display settings start fresh. A **sweep**
export also holds the analysis facts and its impulse response and imports as a sweep again.
A sweep file without a `# sweep_info:` line imports as its transfer function with the delay;
the distortion is dropped and the import says so (`note:`).

### Impulse responses as WAV files

**Import:** a WAV / RF64 file (PCM 16, 24, 32 bit or float, any rate) is read as an impulse
response — a manufacturer's, another analyzer's, or one ac2 exported:

```
ac2 trace import genelec-8030.wav                      # mono
ac2 trace import ir-stereo.wav --channel 2             # channel 2 of a multi-channel file
ac2 trace import room.wav --gate 10ms --gate-pre 1ms   # only 1 ms before … 10 ms after the peak
ac2 trace import a.wav --keep-delay                    # phase re the file's first sample
```

In the app, palette *Import trace…* (CSV, analyzer text, IR WAV); a WAV (recognised by
its header) opens the **Import impulse response** dialog. The app reads the file and sends
it, so it works with a remote daemon.

| field | options | CLI |
|---|---|---|
| Channel (multi-channel files only; the hint names the file's channels, rate and length) | 1 … n, default 1 | `--channel` |
| Time zero | **IR peak (bulk delay removed)** · file start (keep delay) | `--keep-delay` |
| Gate | **off** (the whole file) or a length after the peak, `10 ms` | `--gate` |
| Before the peak | **1 ms**, used with a gate | `--gate-pre` |
| Resolution | 1/12 · 1/24 · **1/48** · 1/96 oct | `--resolution` |

- The result is a **transfer** trace (no coherence), with the IR itself in the IR views.
- **Delay:** by default t = 0 is the IR's peak, found between samples, so the phase is the
  speaker's own (as after the delay finder). `--keep-delay` keeps the file's first sample,
  for IRs exported against one timing reference.
- **Window:** the whole file, unwindowed (exports are usually windowed already); `--gate`
  cuts around the peak with soft edges to keep a room out. Below what the window resolves,
  the resolution marker shades the response.
- A file longer than 65536 samples (1.37 s at 48 kHz) is cut around its peak (said so).
- `ac2 trace show` lists the file facts: `IR · channel 2 of 2 · 48 kHz · 16384 samples ·
  peak at 2.710 ms · whole file · phase re peak`.

**Export:** `ac2 trace export <trace> --wav out.wav` (or the app's export prompt with a
`.wav` path) writes the IR of a sweep, transfer capture or imported IR as mono 32-bit float
at its sample rate; an imported file exports back sample for sample. Traces without an IR
(an RTA, a text import) have nothing to export.

### Mic curve on a stored trace

A trace captured without a mic curve (no curve yet, no mic name, curve off) can be corrected
afterwards. A **sweep** gets its input's curve this way automatically when stored
(`mic M30 (curve M30 applied after capture, …)`), removable like any other.

- **CLI:** `ac2 trace mic <trace> "MM1 34804" --label 90°` (label optional when the mic has
  one curve); `ac2 trace mic <trace> none` removes it.
- **App:** select the trace, palette *Mic curve on the selected trace…* (prefilled with its
  mic), add the label (*MM1 34804 90°*).

Like smoothing it is a display setting:

- the stored curve stays as measured; the correction (0 dB at the calibrator frequency, else
  1 kHz) is applied to the magnitude when shown — never to phase or IR — and `ac2 trace
  show` reads `mic MM1 34804 (curve 90° applied after capture, 0 dB at 1000 Hz, file …)`;
- the curve's points are kept with the trace, so later changes to the mic library do not
  change it;
- a sweep's distortion is corrected too: Hk at f is the ratio of what the mic picked up at
  k·f to f, so it moves by c(k·f) − c(f), and THD is re-summed;
- the export keeps the measured columns and says so (`# mic: … (curve: 90°, applied after
  capture as a display edit, not in the columns; …)` plus a `# mic_curve:` line); a live
  capture's reads `(curve: 90°, in the columns; …)`;
- a trace captured **with** the curve in its columns refuses a second one;
- averages and math channels combine the corrected curves.

## Sweep measurement: response and harmonic distortion

A sweep measures a speaker's response and its **harmonic distortion** (H2 … H5 and THD vs
frequency) in a few seconds. The generator plays a synchronised exponential sine sweep on
the speaker's output and the loopback output; ac2 divides mic by reference and separates the
harmonics, which arrive before the linear impulse response
([sweep-distortion.md](design/sweep-distortion.md)).

The sweep starts about two octaves below the asked start at a rising level, reaching full
level at the asked start, so a switch-on transient does not read as distortion; results are
reported from the asked start.

> Mind the loudspeaker's excursion with a very low start: the extension plays lower still,
> though below full level.

### Making and running a sweep (app)

A sweep is a **measurement**. **Shift+S** (palette *New sweep measurement…*) opens its
dialog:

| Field | Choices |
|---|---|
| reference | the session's loopback input; without a loopback mapping it reads "choose the reference" and nothing is made until one is picked |
| mic, output | by name (the session's loopback output always plays too) |
| **level** | typed, no default |
| from, to | typed, 20 Hz and 20 kHz by default |
| duration | 1 s quick look, 3 s default; 6 s and 12 s lower the noise floor |
| repeats | each doubling lowers the floor by 3 dB |
| silence after | 1, 2, 4, 8 s: must hold the room's decay ([room parameters](#room-parameters-iso-3382-1)) |
| LF harmonics | standard (default) or fine: [low-frequency harmonics](#low-frequency-harmonics-lf-harmonics-fine) |
| resolution | 1/12, 1/24, 1/48 (default), 1/96 oct: [resolution](#resolution-and-the-resolution-marker) |
| name | |

**←/→** step a choice and stop at the ends; a text field's text is selected on focus
(**Ctrl+A** selects again), so typing replaces it.

1. **Enter** makes the measurement and **plays nothing**. It shows on the sweep pane worked
   in last; with none on screen, a pane of its own is split off the focused pane (as **N**
   splits it) and other panes keep what they show. Only a pane too small to split turns into
   the sweep pane. The sweep pane gets the focus.
2. **Space** on the sweep pane arms a run with its settings, with the safety of any
   stimulus (typed level, the rig's maximum, the lease).
3. **Enter** plays it; **Esc** stops and discards it.
4. After the run the stimulus is off and the lease given back; **Space**, **Enter** run it
   again.

Runs are stored **under the measurement** as *Run 1*, *Run 2* … (F2 renames). *Edit
measurement…* changes the settings for the next run. A run is a stored trace, drawn in the
transfer pane like any capture.

### The Sweep / distortion pane

It shows one run of the sweep measurement on its chip — the selected run when it is one of
that sweep's, else its newest — the fundamental's response above, distortion below; runs
compared (**C**) are drawn over it in their own colours and listed `Run 2 · cmp` in the
legend. An order within the noise is drawn dashed
at its own floor; the shading is the noise under every order's floor. A distortion value is
shown only where it is at least 6 dB above the noise in its window; elsewhere it reads
`< −72.0 dB` (`< 0.0251 %`). Lower the floor with repeats or a longer sweep, not with more
level than the speaker should take.

Behind the measurement's orders, **dotted and faint** in each order's colour, are the
reference input's own (*reference H2* … in the legend's *reference* mark, the cursor readout
and `ac2 ir`): the loopback's distortion against the ideal sweep, drawn where it is valid.
It is the converters' and the path's share. Where an order reads within 6 dB of the
reference's own, a note along the bottom of the plot says so (*H2 within 6 dB of reference
H2 at 12 points, 100 Hz – 1.00 kHz*): there the system's harmonic cannot be told from the
interface's. **E** hides the reference's lines in the focused pane (and shows them again);
the note stays.

| Key | Does |
|---|---|
| the **dB \| %** toggle in the title (or Settings › Display › Distortion in) | distortion in dB re fundamental or percent (log axis), readouts included |
| **G** / **Shift+G** | response & distortion → **impulse response** (harmonics' impulses marked) linear → log → ETC → **room parameters** table alone, as large as it fits (read across a room; **W** maximises) → back |
| **Shift+I** | straight to the impulse response and back |
| **E** | the reference input's own harmonics shown / hidden in this pane |
| **V** / **Shift+V** | step through the pane's sweep's runs, selecting each |

### Low-frequency harmonics (LF harmonics: fine)

At the lowest columns (harmonics below about 1 kHz) every order normally shares one window,
short enough for H5. That gives the lowest floor, but at 25 Hz one resolution cell is about
40 % of the fundamental: a harmonic changing steeply there (behind a roll-off) reads its mean
over the cell, a few tenths of a dB high, and columns below about 20 Hz are not reported.

**LF harmonics: fine** puts each order in its own longest window. For sub and LF distortion
work:

- finer LF harmonics, reported below 20 Hz when the sweep starts there (a 5.5 s sweep from
  10 Hz reads H2 from 10 Hz);
- a **higher floor** at those columns (H2's window is several times longer, its noise with
  it);
- above those columns, identical to standard;
- on a digital path that aliases (a DSP clipper without oversampling) the floor there can
  read folded products and hide a harmonic standard shows;
- a **longer silence after the sweep** (up to four of the longest window), so it is a
  setting of the measurement.

> A longer sweep lengthens every window and lowers the floor: 6 s or 12 s with fine gives
> both resolution and a usable floor at the bottom.

Runs read *… · LF harmonics fine* in the trace list.

### Progress strip

While a sweep runs (from any client or the CLI), a strip over the bottom of the panes
(never resizing them; not in full screen) shows the name and level, *sweep 1 of 2*, a bar
and the time left, then *analysing…*. Its **Stop (Shift+Esc)** button, like **Shift+Esc**
or **Esc** (no window open), fades out, disarms and discards the run.

### Sweeps from the CLI

```sh
ac2 meas new sweep --ref 2 --meas 1 --out 1,2 --level -50dbfs --name "Genelec 1 m" \
    [--from 20hz --to 20khz --duration 3s --repeats 1 --gate 5ms --tail 2s \
     --lf-harmonics standard|fine]
ac2 sweep run "Genelec 1 m"
ac2 ir capture --ref 2 --mic 1 --out 1,2 --level -50dbfs    # same flags; --name names the run
```

- `ac2 ir capture` runs the sweep measurement with exactly those settings, making one
  (`Sweep 1`, …) when there is none; the same command again is its next run.
- Like `gen`, both arm and wait: **Enter** plays, **Esc**/**q**/**Ctrl-C** stops. The daemon
  disarms once the sweep has played.
- It prints THD at 100 Hz, 1 kHz and 10 kHz and each order's highest point (`--json`: JSON
  lines).
- `ac2 trace export <sweep> --csv out.csv` writes every curve (response, each order and its
  floor, THD), the analysis facts and the IR; `ac2 trace import` restores the sweep.
- With a mic curve in use on its input the sweep shows it corrected
  ([after capture](#mic-curve-on-a-stored-trace)); export columns stay as measured.

### Arrival

A sweep's arrival is its IR peak re the reference, interpolated band-limited to about a
thousandth of a sample on a clean path, shown to the µs (`arrival 0.004 ms`). Phase, the
IR's t = 0 and the export's `delay_ms` and `sweep_info` `arrival` refer to it, so a
sub-sample delay does not show as phase lag rising with frequency
([subsample-arrival-group-delay.md](design/subsample-arrival-group-delay.md)).
`ac2 trace delay-diff A B` prints the difference of two traces' delays to 0.1 µs with the
path length (`+3.7 µs · +1.3 mm @ 20 °C`; `--temp` sets the air temperature).

### Room parameters (ISO 3382-1)

Every sweep computes EDT, T20, T30, C50, C80 and D50 per octave (and one-third octave) and
broadband ([room-metrics.md](design/room-metrics.md)). The sweep pane's IR view shows the
octave table under the plot when tall enough; the room view (**G** from the IR) shows it
alone in large type. `ac2 ir metrics <sweep> [--third] [--json]` prints it; `ac2 ir capture`
prints the octave table; the CSV export carries it.

| Shown | Means |
|---|---|
| `noise` | the decay meets the noise too soon: T30 needs 45 dB of decay range, T20 35 dB, EDT and C/D 20 dB |
| `short` | the decay is too short for that band's filter |
| `—` | no decay, e.g. an anechoic measurement |
| T30 with `*` | a curved decay (more than 10 % above T20) |

The silence after the sweep must hold the decay: **Silence after** or `--tail 4s`; a 2 s
hall wants 4 s.

## Sessions

The measurements and traces on the daemon are the **working state**. It belongs to a saved
session (a name in the daemon's session directory, a folder on the daemon's machine, or a
file on your computer) or is **untitled**. The top bar shows its name and, in the warning
colour, *unsaved changes*. **Ctrl+S** saves it where it came from (untitled: *Save as*).
Click the name, **Alt+S** or palette *Sessions* opens the Sessions dialog
([sessions.md](design/sessions.md)):

| Row | Does |
|---|---|
| **New session** | no measurements or traces, then the **Audio I/O** settings for the new session's interface and channels |
| **Open…** | a session file (`.ac2session`) or folder on this computer (home, USB sticks and other removable media) in a browser with finger-sized rows |
| **Save** | back to where it came from; untitled: asks for a name first |
| **Save as…** | a new session on the daemon; it then belongs to that one |
| **Export…** | an `.ac2session` file on this computer; a session opened from a file saves back to it |
| **Recover previous: …** | brings back what the last New or Open replaced while it had unsaved changes |
| *Recent* | the daemon's saved sessions and files opened on this computer, newest first |

Backspace goes up a folder; Esc goes back or closes.

- **New** and **Open** ask **Save / Discard / Cancel** with unsaved changes (**Save as…** in
  place of Save when the work is untitled). Discarded changes stay recoverable until the
  next New or Open replaces something.
- Quitting an app that hosts its own daemon asks **Save** (**Save as…**) / **Quit without
  saving** / **Cancel**. Changes left unsaved come back from the autosave when the app starts
  again; on the simulated rig, which keeps no autosave, they are lost.
- **Save as** onto an existing session asks first (**Cancel** is the default).
- An opened session comes up disarmed and keeps the open audio I/O.
- A session of another format version is refused, naming the version, and the dialog offers
  **Import its traces** (they arrive as imported traces; nothing is converted). For its
  measurements too, use `ac2 session salvage`.

| CLI | Does |
|---|---|
| `ac2 session status` | the audio I/O and the working state: name, *unsaved changes*, when saved, what could be recovered |
| `ac2 session new [--discard]` | new, empty measurements and traces |
| `ac2 session save` | saves to the session it belongs to |
| `ac2 session save-as <session> [--force]` | saves as another session; `--force` replaces an existing one |
| `ac2 session load <session> [--discard]` | opens one, disarmed (`load`, because `session open` opens the audio I/O) |
| `ac2 session recover` | brings back what the last `new` or `load` replaced |
| `ac2 session salvage <session>` | brings a session of another format version into the current working state (below) |
| `ac2 session export <file> [--session <name>]` | writes an `.ac2session` file here: the working state or a saved session |
| `ac2 session import <file> [--name <n>] [--force]` | stores a file or folder from here as a saved session on the daemon, without opening it |
| `ac2 session list` | the daemon's saved sessions |

`<session>` is a name, a folder on the daemon's machine, or an `.ac2session` file here (sent
in checked chunks). `new` and `load` with unsaved changes are refused (exit code 4) unless
`--discard`. A network client never names folders on the daemon's machine.

**Salvaging an older session.** After an update that changes the session format, the daemon
sets its old autosave aside as `autosave.vN` and starts empty. `ac2 session salvage <folder
or .ac2session file>`, where that path is readable, rebuilds it on top of what is there:

- each measurement with its name, kind, settings, colour, own pane layout, and a transfer's
  delay and tracking;
- each stored trace with its name, colour, slot, offset, polarity, nudge, smoothing and
  lock, under its measurement; math channels on the restored items;
- measurements that were running are started when the open audio I/O captures their
  inputs (open the audio I/O first);
- panes and the selection that showed a measurement come back to it once it is recreated.

The old file is read setting by setting: a setting dropped or added since is named in the
summary, which lists every item as restored or skipped. Not brought back: an SPL meter's
per-second Leq history (the log stays in the old folder), an after-capture mic curve, a
delay's offset from the found arrival (the delay itself is applied) and trace provenance.
Run it once (a second run adds a second copy). With a session name, or a path only on the
daemon's machine, only traces are imported.

A session holds the measurements and traces (slots, display edits, sweep distortion and
IRs) and each SPL meter's per-second log (a running meter is not an unsaved change). Not
**calibrations**: they describe the machine's hardware and stay in its calibration store.
An `.ac2session` is a compressed archive of a session folder with a checksum; the daemon
refuses one that is damaged, larger than 64 MiB, or unpacks to more than 512 MiB.

### Autosave

A stand-alone daemon (`ac2d`, `ac2 daemon start`, the user service) **autosaves** the same
content 1.5 s after the last edit of a burst (at most 10 s after the first) and once more
when it stops. On the next start it loads it as `session load` does — disarmed, no audio
session (restored measurements wait for one), same session name and *unsaved changes* — and
logs what was restored. The daemon inside the app autosaves to `autosave-app/`; a simulated
rig does not.

The top bar shows the state (`ac2 status` and `ac2 session status` print it too):
*autosaved 5 min ago*, *saving…*, or *autosave failed: <reason>* in the warning colour
(it retries; hover for the whole reason).

Files, in the ac2 data directory (`~/.local/share/ac2` on Linux, `~/Library/Application
Support/ac2` on macOS, `%APPDATA%\ac2\data` on Windows):

| | |
|---|---|
| `autosave/` | the current autosave (a session directory); inside it `session.prev.json` is the one before, used should the newest be damaged |
| `autosave.v<N>/` | an autosave of another session format, set aside at start, not deleted |
| `autosave.damaged/` | an unreadable autosave, set aside |
| `autosave.unrestored/` | what `ac2d --no-restore` did not load |
| `autosave.replaced/` | what the last New or Open replaced with unsaved changes (**Recover previous**) |

Any of these loads by path: `ac2 session load ~/.local/share/ac2/autosave.unrestored`.

When an SPL meter's log cannot be written (a full disk, a read-only or missing directory)
every pane shows **LOG NOT SAVED · disk full** in the warning colour, and `ac2 status` /
`ac2 session status` print `log          NOT SAVED: SPL log of Main not saved for 12 min:
disk full`. The meter and its Leq history on screen are complete; the file misses the
seconds since then until a write succeeds again (tried every 30 s and with each autosave),
when the banner goes by itself. Free space and wait, or save the session elsewhere.

The app's own preferences (`ui.toml`) that cannot be written stay unsaved and are tried
again every 30 s; the first failure is a notification.

To spare SD cards and batteries, a write adds only changed traces; an SPL meter's
per-second log is appended every 30 s and synced every 5 min and at stop (after a power cut
it is back to its last few minutes; after a daemon crash, 30 s).

After a power cut a state file may be torn. The daemon never refuses to start over one: it
sets the file aside as `<name>.damaged-<time>` (never deleted), reads the copy before the
last write where it keeps one (`rig.json.prev`, `calibrations.json.prev`) or starts without
it, and goes on metering. In `rig.json` a part that cannot be read (one rig meter, the
outputs, the system max level…) costs only that part: the rest is read, and the copy set
aside keeps what was lost. An SPL log cut mid-row keeps every whole row and carries on
after it (nothing is reported). A damaged trace or SPL log costs only itself (moved to
`<autosave>.damaged-files/`). The app shows **FILE DAMAGED · what** until the daemon
restarts, and `ac2 status` prints a `damaged` line for each file with what was done.

| Flag | Does |
|---|---|
| `ac2d --no-restore` | starts empty |
| `--autosave <dir>` | puts the autosave elsewhere (one daemon per directory) |
| `--no-autosave` | keeps everything in memory only |

### Watchdogs under systemd

Under systemd `ac2d` and `ac2-ui` tell the unit when they are ready and keep a watchdog
fed, so a frozen program is restarted instead of showing a stuck value: `ac2d` pings from
its control loop (what answers clients and sends keepalives), `ac2-ui` after each frame it
draws (it draws at least once a second while a watchdog is set). A unit for them:

```ini
[Service]
Type=notify
WatchdogSec=10          # ac2d; 5 for ac2-ui
TimeoutStartSec=60      # ac2-ui: its first frame waits for the GPU
Restart=always
```

`NotifyAccess=main` (the default) is right when the program is the unit's main process,
also through a wrapper script that `exec`s it. Without `NOTIFY_SOCKET` (a desktop, macOS,
Windows) none of this does anything. Design: `docs/design/watchdogs.md`.
| `--rig-logs <dir>` | where rig SPL meters keep their logs ([rig measurements](#rig-measurements); not affected by the three above) |

## Recording and replay

A **recording** keeps exactly what the converter delivered on the session's inputs, with
what the operator was doing, so measurements can be re-run later (another machine, build or
settings) and a rig problem examined after the show. It is not a programme recorder: no
compression, no outputs ([raw-capture.md](design/raw-capture.md)).

### Recording

- **App:** palette *Record* (type `rec`) records every input; it stops by itself after the
  time on Settings › Recording (1 h default, up to 8 h). The top bar shows `REC 1:23 ·
  23.0 MB` (audio time in the file), then `recorded show · 1:23 · 23.0 MB (time limit)` or
  the failure.
- **CLI:** `ac2 rec start --max 10min [--in 1,2] [--max-size 2GB] [--name show]` (`--max`
  required), `ac2 rec stop`, `ac2 rec status`, `ac2 rec list`.

One recording at a time. It ends and is kept on stop, at its time or size limit, on a write
error (disk full: everything before is kept), when the session closes or reopens, and when
the daemon stops. Gaps (an xrun, a slow disk) are never filled: the sidecar says where
samples do not follow each other.

Files go to `recordings/` in the ac2 data directory on the daemon's machine (`ac2d
--recordings <dir>` elsewhere); a name is never reused:

| File | Holds |
|---|---|
| `<name>.wav` | 32-bit float, the inputs in order, bit for bit as captured; RF64 past 4 GiB |
| `<name>.ac2rec.json` | the sidecar: device, inputs with their mics and roles, calibrations, every measurement and generator change during the recording, and the gaps |

### Replay

**Replay** opens a session on a recording instead of a device: the running measurements
analyse it with the recorded device's calibrations and mic curves, no outputs.

- **App:** palette *Replay recording…* (a name or a path).
- **CLI:** `ac2 session replay <name|path> [--fast]` (a name from `ac2 rec list`, or a
  `.wav` / `.ac2rec.json` path). Real time by default; `--fast` goes as fast as the
  measurements allow, never dropping audio.

Replay reproduces samples, blocks and gaps exactly: block-averaged results (transfer,
spectrum, SPL) match the live run to rounding; anything paced by wall time (RTA averaging,
the per-second SPL log) matches closely only in real time.

**A recorder's WAV:** `ac2 rec import ZOOM0001.WAV [--name flat4] [--dir DIR]` turns a 16-,
24- or 32-bit PCM or float WAV into a recording `session replay` plays; it runs locally, no
daemon needed. Used for the [band transfer](#4-a-recorder-at-the-place).

## Calibration and SPL

Inputs carry a **mic name** (**N** on the input in Settings › Audio I/O or Calibration,
`ac2 session open … --mic 3=M30`, `ac2 session inputs --mic 3=M30`). The daemon's machine
stores, in its calibration store:

- **Sensitivity** (dB SPL of 0 dBFS): per device, input and mic.
- **Mic curves** (the **mic library**): per mic, any number, each with a short **label**.

#### Sensitivity

It calibrates the whole chain, preamp gain included: **change the gain and calibrate
again**. SPL is computed from the raw input level with it.

| Method | How |
|---|---|
| **acoustic** (the reference method) | a 94 dB (or 114 dB) calibrator on the mic. CLI: `ac2 cal spl --input 3 --ref 94db` (`--freq` when the calibrator is not 1 kHz). App: **C** on the input in the Calibrations view. |
| **electrical** ([below](#calibrating-without-a-calibrator-electrical)) | no calibrator, but a true-RMS voltmeter at the input and the mic's data-sheet sensitivity, with a stated uncertainty (±1 dB). **E** in the Calibrations view, or `ac2 cal electrical`. |

The app's acoustic dialog shows the input's level live and takes the mic name (prefilled),
the calibrator's level (←/→ 94 / 114 dB) and tone (1 kHz / 250 Hz). Enter reads and stores;
while the level is settling it says so and Enter tries again. A calibration made with
another mic or input is used but shown as such. An acoustic calibration replaces an
electrical one, never the other way round unless you say so.

#### Mic curves

A measurement mic often comes with one file per incidence angle (0° pointing at the source,
90° grazing); the wrong one is a few dB of error at high frequencies.

- `ac2 cal curve import 449350_34804_90Grad.txt --input 3` imports a file as a curve of
  input 3's mic (`--mic NAME` names the mic directly).
- The label comes from the file (an angle in its header or name: *90-degree-curve*,
  `_90Grad`, `0deg` → *90°*, *0°*; else the file name) or `--label`;
  `ac2 cal curve rename --mic "MM1 34804" 0° "on axis"` renames it.
- A sensitivity the file states (*15.0 mV/Pa = −36.5 dBV*) is shown as the data sheet value
  and used only as the default of an electrical calibration.
- Curves follow the mic name to any input and device.

#### Which curve is in use

Chosen per input, explicitly:

- **CLI:** `ac2 cal use 3 90°` (or `off`); `ac2 session inputs --mic 3=M30 --curve 3=90°`
  sets several inputs at once; `ac2 session inputs` alone lists them.
- **App:** **←/→** on the input's row in Settings › Audio I/O or the Calibrations view,
  palette *Mic curve on input N…* (*3=90°*) or *Mic curve: next curve on the selected
  measurement's input*.

ac2 never guesses: a mic with exactly one curve uses it on every input naming the mic; with
several and none chosen, none applies and the input says *choose: 0°, 90°*.

**The rule:** an input's curve in use applies to every measurement reading the input —
transfer, spectrum, RTA, SPL meter, Leq and band Leq, and sweeps. It corrects levels (the
magnitude; a sweep's harmonics at their own frequencies, and THD); phase, coherence and IRs
stay as measured. A sweep's analysis works on the raw recordings, so its trace gets the curve
after capture, normalised as the live measurements are
([Mic curve on a stored trace](#mic-curve-on-a-stored-trace)): a sweep and a live transfer of
the same input read the same. A change applies to running measurements at once (no reset).
Every corrected readout names its curve — the sidebar input label, the transfer / spectrum /
RTA / SPL captions (*mic curve: MM1 34804 90°*), `ac2 cal list`, `ac2 status` — or says why
there is none (*mic curve off*, *no mic curve stored for MM1 34804*, *mic curve 90° not
stored for MM1 34804*). Captured traces keep which curve their columns carry (label, file,
content hash; the export header says it).

### The Calibrations view

The **Calibration** page of Settings (palette *Calibrations…*; opens on the selected
measurement's input) lists what each input uses, every mic with its curves (file, points,
range, data sheet sensitivity, inputs using it) and every sensitivity calibration (device,
input, mic, calibrator level and frequency, reading, age).

| Key | Does |
|---|---|
| **↑/↓** | move |
| **←/→** | choose an input's curve |
| **N** | names the mic on an input |
| **I** | imports a curve file for the focused mic (type the path) |
| **R** | renames a curve |
| **C** | calibrates an input with a calibrator ([above](#sensitivity)) |
| **E** | calibrates an input electrically ([below](#calibrating-without-a-calibrator-electrical)) |
| **Delete** (twice) | deletes a curve or a sensitivity calibration |

CLI: `ac2 cal list`, `ac2 cal curve rm --mic NAME LABEL`, `ac2 cal rm --input 3`.

A calibration store of another format version is set aside (renamed `calibrations.json.v1`,
`.v2`, …, never deleted) and the daemon starts with an empty store: calibrate and import the
curves again (the old file shows the mic and file names).

### What the calibration labels say

Every calibrated readout names what its dB SPL rests on (the SPL footer, Leq caption,
spectrum / RTA captions, the Calibrations view, `ac2 cal list`, `ac2 status`):

| label | meaning |
|---|---|
| `cal 94 dB · 3 h ago` | acoustic: a 94 dB calibrator on this mic on this input, 3 h ago |
| `verified · 94.0 dB SPL at 1.00 kHz · 3 h ago` | the same, written out (Calibrations view, Settings › Audio I/O) |
| `electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB · 2 h ago` | electrical, a voltmeter across the connected, powered mic's pins 2–3; `data sheet` when the sensitivity came from the mic's curve file, nothing when it was typed (the full record in the Calibrations view and `ac2 cal list` says `typed`); ±1 dB the stated uncertainty |
| `electrical cal (injected, 10.0 mV/Pa) ±0.5 dB` | electrical, a generator in place of the mic, phantom off |
| `electrical cal (in-line, data sheet 15.0 mV/Pa, loaded 330 Ω → 2.0 kΩ −1.3 dB) ±1 dB` | electrical with the loading counted: the mic's 330 Ω output impedance into the preamp's 2 kΩ input takes 1.3 dB off its open-circuit sensitivity ([Loading](#loading-the-preamps-input-impedance)); the full record (`ac2 cal list`) says `… · loaded 330 Ω → 2.0 kΩ −1.3 dB · ±1 dB` |
| `cal from other mic / input`, `from M30 on in 2 · …` | made with another mic or on another input: used, but may not hold here |
| `uncalibrated` (values in dBFS) | no calibration for this device, input and mic |
| `· mic curve` after any of these | a mic curve is in the readout too (its label is in the caption) |

Leq limits are judged only on a calibrated input, whichever method.

### Calibrating without a calibrator (electrical)

With a true-RMS multimeter (or an Analog Discovery 2) instead of a calibrator, ac2 reads the
input's level while you measure the voltage at the same input; the mic's data-sheet
sensitivity (mV/Pa) turns that into dB SPL:

    V_FS = V / 10^(L / 20)                 volts at 0 dBFS (V measured, L read in dBFS)
    dB SPL = dBFS + 20·lg(V_FS / (S · 20 µPa))     S = mic sensitivity in V/Pa

Example: 15.0 mV at −40.0 dBFS → 0 dBFS = 1.500 V; at 15.0 mV/Pa that is 100 Pa, so 0 dBFS =
134.0 dB SPL, and 1 Pa (94 dB) reads 94.0.

- It counts as a calibration everywhere and is labelled as such
  (*electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB · 2 h ago*).
- The ±1 dB is mostly the data sheet's (capsule tolerance typically ±0.5…1 dB; with an
  individual calibration sheet state `--uncertainty 0.5db`); the meter adds a little.
- A later acoustic calibration replaces it; it replaces an acoustic one only when you say
  so (`--replace-acoustic`, or Enter twice in the app).
- Give the mic's output impedance and the preamp's input impedance too
  ([Loading](#loading-the-preamps-input-impedance)): without them SPL reads low by the
  loading, often 1 dB or more.

#### Loading: the preamp's input impedance

A data sheet states the sensitivity **open circuit**: the voltage per pascal with nothing
connected. In use the mic drives the preamp, and its output impedance and the preamp's input
impedance divide that voltage:

    S_eff = S · Z_in / (Z_in + Z_out)        loading = 20·lg(Z_in / (Z_in + Z_out)) dB

ac2 uses `S_eff` in place of `S`. This holds for both connections: in-line, the meter reads
the loaded voltage, but it is paired with the open-circuit sensitivity, so SPL reads low by
exactly the loading unless you state it.

Example: a beyerdynamic MM1 (15 mV/Pa, 330 Ω) on an RME Fireface 400 mic input (2 kΩ):
`S_eff` = 15 · 2000 / 2330 = 12.88 mV/Pa, a loading of −1.33 dB. Without it, the MM1 read
1.3 dB below a sound level meter.

- **Mic output impedance:** the mic's data sheet, "nominal impedance" or "output
  impedance" (MM1: 330 Ω).
- **Preamp input impedance:** the interface's manual, "mic input impedance" (Fireface 400:
  2 kΩ). Use the mic input's figure, not the line or instrument input's.
- CLI: `--mic-impedance 330ohm --input-impedance 2kohm` (also `330Ω`, `2kΩ`), always both;
  app: the *Mic output impedance* and *Preamp input impedance* fields (`330 ohm`, `2 k`;
  blank = no correction), the correction shown live under them.
- The calibration's label names it (`…, loaded 330 Ω → 2.0 kΩ −1.3 dB`). The record keeps
  the data sheet's mV/Pa and both impedances; its dB SPL of 0 dBFS counts the loading.

#### In-line (the usual way: the mic stays connected and powered)

1. Put an XLR breakout (in-line adapter with test points) between mic cable and input.
   Phantom stays **on**.
2. Set the preamp gain you will measure with; do not change it afterwards.
3. Play a steady **1 kHz** sine through a speaker at the mic, loud enough that the meter
   reads well above its range floor (a handheld DMM: ≥ 5–10 mV) with the input below
   −3 dBFS (your own tone source, or ac2's generator). Mic and speaker must not move.
4. Meter on **AC volts** (true RMS; AC coupling also blocks the phantom voltage), lowest
   fitting range; check its accuracy at that range and 1 kHz. Measure **between pins 2 and
   3 only**.

   > **Warning:** phantom power puts +48 V on both pins 2 and 3 against pin 1. Never
   > measure to pin 1, never short pins together (that is what the breakout is for).
5. While the tone plays: palette *Calibrations…*, the mic's input, **E**; type the voltage
   (*15.03 mV*), check the sensitivity (prefilled from the mic's curve file when it states
   one; else empty, and the dialog asks you to type it: *15.0 mV/Pa* or *−36.5 dBV/Pa*) and
   the two impedances (*330 ohm*, *2 k*),
   **Enter**. CLI:
   `ac2 cal electrical --input 2 --volts 15.03mv --mic-impedance 330ohm --input-impedance 2kohm`
   (`--sensitivity 15.0mv/pa` if the curve file has none).

   ac2 refuses while the level is not steady (retry after a few seconds), without signal,
   when the input clipped, above −3 dBFS or below −70 dBFS.

In-line, the mic loads the preamp as in use; the loading correction still applies, because
the data-sheet sensitivity it is paired with is open circuit.

#### Injected (a generator in place of the mic)

1. > **Switch phantom power OFF on that input first.** 48 V on the XLR can damage the
   > generator; ac2 cannot switch phantom power.
2. Wire the generator between pins 2 (hot) and 3 (cold, also to pin 1 for an unbalanced
   generator), gain as you will measure with.
3. Play 1 kHz near what the mic gives (tens of mV), measure the voltage at pins 2–3, and
   run `ac2 cal electrical --method injected --input 2 --volts 15.0mv --mic-impedance 330ohm
   --input-impedance 2kohm` (app: ←/→ on *Measured* chooses injected). The full-scale
   voltage is the input's, whatever drives it; the mic will load the input in use, so state
   its loading here too.
4. Disconnect the generator, plug the mic back in and **switch phantom back on**.

#### Tips

Small voltages are hard to read on a handheld meter. **Divider tip:** set the generator to
about 1 V, measure that precisely, and feed the input through a precise divider (0.1 %
resistors, 10 kΩ : 100 Ω ≈ 1 : 101); the input voltage is the measured one times the ratio.
Measure the divider's output loaded by the input when you can.

A tone other than 1 kHz is accepted (`--freq 400hz`, for a meter specified only to 400 Hz):
the sensitivity is still the capsule's at 1 kHz, so this assumes the preamp is flat between
the two (ac2 notes it). The mic curve stays normalised at 1 kHz.

### SPL meter

The **SPL meter** shows the sound level in the SPL pane or the terminal: time weighting
**Fast**, **Slow** or **Impulse** (IEC 61672-1: F and S exponential averages of the squared
signal with 125 ms and 1 s; I, from IEC 60651, 35 ms with peaks falling at 1.5 s), frequency
weighting **A**, **C** or **Z**, with Leq, LCpeak, Lmax and Lmin. A new meter reads
**A-weighted, Fast** (`LAF`) unless chosen otherwise.

#### Three views, G steps them

**G** steps meter → Leq windows → meter + Leq (→ bands, with a band meter) → meter; the
palette has each by name (*Meter view*, *Leq view*, *Meter + Leq view*, *Band Leq view*).

| View | Shows |
|---|---|
| **meter** | the number and the meter's statistics ([below](#the-number)) |
| **Leq windows** | the windows as columns or tiles ([next section](#leq-windows-and-limits)) |
| **meter + Leq** (where a new meter starts) | the meter's number across the top third (with `LAF · dB SPL` and the live bar, no statistics) and the Leq windows below, under one caption: the meter's name, the run and the calibration |

**W** twice (or W, then **F**) makes it the stage view. On a short pane the number and its
name share one line; with less than about 120 px under the caption the number gives way to
the windows.

#### The number

The current time-weighted level, as large as the pane allows. Under it: the name and unit
(`LAF · dB SPL`, `dBFS` uncalibrated); a slim live bar (30 … 130 dB SPL or −100 … 0 dBFS);
the meter's statistics under *meter since 4:01 · R resets* — `LAeq`, `LCpeak`, `LAFmax`,
`LAFmin` in its weightings; the calibration. The statistics run since the meter's start or
the last **R**; the Leq windows are separate and never reset by R. Full screen, the grey
calibration line shows only with STALE or STOPPED.

**Readable, not flickering.** The number updates twice a second with F and I, once with S,
as a hand-held meter does; the bar moves every frame. The reading is the time-weighted level
at that instant (never averaged in dB). `spl_hold_ms = 250` in `ui.toml` (or Settings ›
Display › SPL number holds) sets another period (100 … 10000 ms).

#### Changing the weightings

**Shift+F** steps time weighting Fast → Slow → Impulse, **Z** frequency weighting A → C → Z;
the palette has each by name. The change applies at once and is kept with the meter. With
the Leq windows showing, a message names the new reading (*FOH SPL: LCS*); the windows keep
their own weightings. **Nothing restarts:** the meter measures every combination all the
time, so the new one reads settled from the first frame and its Lmax, Lmin, Leq and Lpeak
cover the same interval. The Leq windows and per-second log carry on.

#### SPL from the terminal

```sh
ac2 spl set --weight c --time slow       # --meas or --input when there is more than one meter
ac2 spl watch --input 3 --weight a       # --json: one JSON line per update; --for 10s stops on its own
```

With `--input` the command runs its own meter while it runs; `--meas` shows an existing one.

#### Rig measurements

An SPL meter can belong to the rig instead of the session (its `scope` is `rig`). A rig meter
watches a noise limit whatever happens to the session: loading a session, starting a new one
or the daemon restarting leaves it running, with its limits, its log and its Leq history. It is
kept with the daemon's rig settings (`rig.json`), never in a session file, and its log in its
own directory (`ac2d --rig-logs`, default `rig-logs` in the ac2 data directory). Traces stay
the session's: a trace captured under a rig meter is saved with the session as an import.

With **monitor at start** on, the daemon remembers the device of the last session opened while
rig meters exist, and opens it again by itself when it starts, capture only (no outputs, so
nothing can be played), so the rig meters meter with no app or terminal connected. Closing
the session, or opening another device, while rig meters run is allowed; the reply warns
(the CLI prints it, the app shows it as a toast even with warning toasts off, the daemon
logs it) that the rig meters stop metering, or meter the new device's inputs.

From the terminal:

```
ac2 meas new spl --input 1 --name Stage
ac2 meas scope Stage rig          # the rig's from now on (`session` makes it the session's again)
ac2 rig monitor-at-start on       # a restarted daemon reopens the device for it
ac2 rig status                    # OSC target, counters, monitor at start, rig meters and cues
ac2 session close
ac2: warning: rig SPL meter(s) Stage (1) stop metering until a session opens again; their OSC cues report fault
```

### Leq windows and limits

Every SPL meter also keeps **rolling Leq windows** (by default LAeq over 1, 5, 10, 30 and
60 min) and a **per-second log** (LAeq, LCeq, LZeq of every second, the last 48 hours). Both
run whether or not anyone watches, and carry on across stop/start, window changes, session
reopen and daemon restart (the log is in the autosave and sessions). How it is computed:
[leq.md](design/leq.md).

#### Reading the columns

The windows show as **columns**, made to read from the stage: one full-height column per
window, shortest on the left, each with a bar filling with the window's Leq, the value in
large digits with its unit and weighting beside it (**dB SPL (A)**, **dB SPL (C)**, **dB SPL (Z)**, or
**dBFS (A)** uncalibrated — never left out, since the meter above may read another
weighting), the window's name at the bottom ("LAeq 30 min", shortened when narrow), and the
limit as a line. A column turns **amber** within the warn margin (3 dB default) and **red**
over the limit; each going over and recovering also shows as a message.

Each column (and tile) says, large: its **state** (OK, NEAR, OVER, or "over in 47 s" when a
filling window is on course to go over); the **headroom**, the highest steady level for the
next minute that keeps the window at or under its limit (**"stay ≤ 101.5 dB SPL"**); or, over and
unable to recover within the minute, **"cooling down in 7 min 30 s"** (time back under if the
level stays at the limit). Smaller: the limit; the window's Leq low on the bar; while
filling, how much there is; **"offline for 1 min 20 s"** when part of the window has no audio
(meter stopped, daemon down, lost samples) — missing audio is not silence: it neither lowers
the Leq nor spends or earns budget. Narrow columns shorten or drop lines.

**One scale:** from 30 dB below the lowest limit to 6 dB above the highest; without limits
or uncalibrated, a 40 dB range following the loudest window in 10 dB steps.

#### A window still filling: the budget

A window still filling (a new log, a window longer than the meter has run) shows its Leq
**so far** ("so far · 12:30 / 30:00") but is judged on its **budget**: the sound energy the
limit allows over the whole window.

- It goes **red** only once that is spent: the window will end over its limit even if all
  is silent from now.
- Before that, a Leq so far above the limit is **amber, "ON COURSE — over in 12 min"**:
  turning down now still keeps it under.
- Its bar shows the budget spent; its headroom is the level that uses exactly what is left
  by the time it is full ("stay ≤ 98.2 dB SPL"; the CLI says "until full: …").

Example: 70 dB against 60 dB limits on a fresh log is ten times the limit's power: the
1 min window is red after about 6 s, the 60 min one after about 6 min. The regulations
define limits on full windows only; red only when going over is certain is ac2's choice for
the time before.

#### Tiles, history and the stage view

- Settings › Display › **Leq windows** (palette *Leq style*): columns
  or **tiles** (a grid with every figure written out).
- Settings › Display › **Leq history strip** (palette *… history strip on / off*): each
  window over time against its limit (dashed), red where over; live, it shows twice the
  longest window (2 min … 2 h) up to the newest second, its last 4 hours rebuilt by the
  daemon for a restarted or second app. A new log clears it.
- **The strip as a graph**, over the whole log the daemon keeps (48 hours): the **wheel**
  zooms time about the pointer, a **drag** pans it, **I** / **O** zoom and **←** / **→** pan
  the focused pane's strip. Looking back, the caption at its top right says
  **`history · 14:02–15:10 · Home: live`** (local times; seconds under 10 minutes, "loading"
  while the span is fetched) and the axis reads the clock; **Home** returns to live. Older
  spans come from the daemon at the strip's resolution: a point per second when zoomed in,
  else each point the **highest** of its seconds, red if any of them was over.
- **The pointer on the strip** reads that time: the second (or `16:42:00–16:43:00 ·
  highest` span) and each window's Leq, limit and state (`LAeq 15 min 101.0 dB · limit
  100.0 · over`). **T** hides it with the grid.
- Settings › Display › **Leq bars**: each column (and band bar) glides to a new reading over
  0.3 s; the number, colours and limit lines change at once. "jump to each new reading"
  turns it off.
- **T** (as on a graph): the colours alone, without OK / NEAR / OVER, "stay ≤" or "limit"
  text; per pane, kept with the layout (see *Grid, labels and cursor (T)*).
- **W**, then **W** or **F**: the **stage view**, nothing but the columns (in meter + Leq,
  the number above them). The grey caption shows there only with the history strip on, or
  with STALE.

#### The run caption

With the history strip on, the caption above the windows says
**`running 2:14:05 since 19:02 · LAeq total 97.8 · offline 12 s`**: time since the log's
first second (kept across app and daemon restarts), its local start time, the energy average
over everything measured (LCeq and LZeq too when a window uses them), and time not measured.
Narrow panes shorten it (`2:14:05 · total 97.8`); after 48 hours it says "last 48 h".
Without the history strip it is not shown; `ac2 spl leq watch` always prints it.

#### A new log (Shift+R)

**Shift+R** in the SPL pane (palette *New SPL log…*) starts a **new log**, for the
show after a loud soundcheck: windows, states, alarms, clock and total start over; windows
and limits stay. It asks first, naming the run that ends; **Shift+R** again starts it (**Enter** does
nothing there), **N** or **Esc** keeps the current log. The ended log can be exported
(`ac2 spl leq export --previous`) until the next new log or daemon restart.

#### Setting windows and limits (Shift+L)

**Shift+L** (palette *Leq windows and limits…*) opens Settings › SPL / Leq: window lengths
and weightings (←/→), limits and warn margins typed in dB (empty: no limit), a **preset**
row and the headroom horizon, and under the windows the **LCpeak** and **LAFmax** limits and
the **position correction**.

| Key | Does |
|---|---|
| ↑/↓ | move between rows |
| Tab | move between cells |
| **Insert** | adds a window |
| **Shift+Insert** | in the band windows: a range line adding one window per band |
| **Delete** | removes one |
| Enter | applies |

**Peak limits** (LCpeak, LAFmax) are over as soon as any second's C-weighted peak (A-weighted
Fast level) exceeds the limit, held 10 s after the last such second so a kick drum near the
limit does not flicker. They show as columns of their own, "highest of the last 10 s".

**States settle before they drop:** a window or peak goes amber or red at once, but comes
back only 0.3 dB under the line or after 10 s under it.

**Position correction:** the difference from your mic to where the limit applies (the
loudest audience spot, measured with pink noise at both places beforehand), e.g. *4* dB, and
for LCpeak when different (DIN 15905-5's K2; LAFmax takes the level correction). Every level then includes it and says so
(*corrected +4.0 dB*, `dB SPL (A) corr.`); limits are judged on it. The per-second log keeps what
the mic measured, with the correction beside each second.

#### Presets

A preset **replaces the windows** with exactly the rule's (windows and limits, and windows
it wants shown without a limit), and sets its peak limits; ←/→ on the preset row previews
each, "none" restores the windows as they were. Editing keeps the preset's windows; the log
carries on and the new windows are rebuilt from it. A preset leaves the position correction
alone. Limits are judged only on a calibrated input; an uncalibrated meter shows dBFS,
marked "not calibrated".

> Informational only, not legal advice: each rule also has a measuring position and duties
> of its own ([leq.md](design/leq.md) lists them).

| preset (`--preset`) | windows and limits |
|---|---|
| DIN 15905-5 (`din15905`) | LAeq 30 min ≤ 99 dB, LCpeak ≤ 135 dB |
| Swiss V-NISSG (`swiss93`, `swiss96`, `swiss100`) | LAeq 60 min ≤ 93 / 96 / 100 dB, LAFmax ≤ 125 dB |
| WHO safe listening, 2022 (`who`) | LAeq 15 min ≤ 100 dB |
| France R1336-1 (`france`) | LAeq 15 min ≤ 102 dB, LCeq 15 min ≤ 118 dB |
| France R1336-1, children up to 6 (`france-children`) | LAeq 15 min ≤ 94 dB, LCeq 15 min ≤ 104 dB |
| Flanders VLAREM II (`flanders-85`, `flanders-95`) | LAeq 15 min ≤ 85 / 95 dB |
| Flanders VLAREM II (`flanders-100`) | LAeq 60 min ≤ 100 dB, LAeq 15 min shown |
| Brussels (`brussels-85`) | LAeq 15 min ≤ 85 dB |
| Brussels (`brussels-95`) | LAeq 15 min ≤ 95 dB, LCeq 15 min ≤ 110 dB |
| Brussels (`brussels-100`) | LAeq 60 min ≤ 100 dB, LCeq 60 min ≤ 115 dB |
| NL covenant, voluntary (`nl-covenant`) | LAeq 15 min ≤ 103 dB |
| NL covenant, ages 16–17 / 14–15 / up to 13 (`nl-covenant-16-17`, `nl-covenant-14-15`, `nl-covenant-13`) | LAeq 15 min ≤ 100 / 96 / 91 dB |
| Finland STM 545/2015 (`finland-545`) | LAeq 4 h ≤ 100 dB, LAFmax ≤ 115 dB, LCpeak ≤ 140 dB |

Wallonia has no preset: its 2018 rule is not in force.

#### Leq from the terminal

Each command takes `--meas` or `--input` when there is more than one meter.

| Command | Does |
|---|---|
| `ac2 spl leq watch` | big numbers; `--json` for one line per second |
| `ac2 spl leq set --preset france --windows 1min --limit 1min=102db` | the preset's windows plus an LAeq 1 min of your own. Several `--preset` give the windows of all, a shared window at the lower limit; without `--preset`, `--windows 1min,5min,c:30s` sets the windows. Also `--warn 3db`, `--horizon 1min`, `--peak-limit lcpeak=135db`, `--position 4db [--position-peak 2db]`. |
| `ac2 spl leq export -o show.csv` | the per-second log as CSV, for the record |
| `ac2 spl leq export --previous -o soundcheck.csv` | the ended log (until the next new log or daemon restart) |
| `ac2 spl leq new --yes --export soundcheck.csv` | a new log, the ended one written first; without `--yes` it only says what would end |

The app writes the same CSV on the computer it runs on: palette (Ctrl+K) **Export Leq log
(CSV, on this computer)…** for the focused (or only) SPL meter, with an editable name such
as `~/ac2-exports/FOH-SPL-leq-2026-10-17-0012.csv` (local time; the folder is made). The
toast says `43200 seconds of FOH SPL written to …`. **Export the previous Leq log…** does the
same for the ended log. Both read the log page by page (`spl.log_get`) without holding up
the app.

### Band Leq: band limits at a neighbour's

An SPL meter can also keep a **band meter**: the Leq of 1/3-octave bands over rolling **band
windows**, each against its own limit, for rules that limit the sound reaching a neighbour
per band. A band window is one band (20 Hz … 10 kHz) with its length, weighting (Z, A or C),
limit, day offset and warn margin; a meter has up to 64 ([band-leq.md](design/band-leq.md)).
Finland's STM 545/2015, for one, limits the hour's unweighted Leq per band 20 … 200 Hz in
rooms for sleeping (night 22–07: 20 Hz 74 dB … 200 Hz 32 dB; day 5 dB higher) and music at
night to LAeq,1h 25 dB — eleven windows, one per band.

Such limits hold **at the receiving place**, while the mic is usually at FOH. A **transfer**
measured once at setup (per band, how much quieter the place is than FOH) moves the limits
to the mic and predicts the place's LAeq. The place is named when measured ("flat 4
bedroom"; "receiving room" until then).

#### Turning it on

**Shift+L**, ↑ from the preset row to the **Band meter** rows:

- ←/→ picks off, **on** or a preset: **Finland STM 545/2015, low frequencies** (eleven LZeq
  60 min windows 20 … 200 Hz with the table's limits, predicted LAeq ≤ 25 dB at night) or
  **… living room** (the same windows without limits; predicted LAeq against 35 dB by day,
  30 dB at night). A preset replaces the windows and predicted window and keeps corrections
  and a measured transfer; any later change reads **on**.
- **Band windows**, one line each: **Band**, the window (**LZeq 60 min**), weighting (all
  ←/→), **Limit (dB)** (empty: none), **Day +dB** (empty: same day and night; a number: the
  typed limit is the night's, 22–07, the day's that much higher), **Warn within (dB)**. Only
  the windows' bands are drawn, judged and alarmed; every band is logged.
- **+** at the **Band windows** (or **Leq windows**) heading adds a window (turning the band
  meter on): the focused window's length, weighting and margin on the next band not yet a
  window of that kind (20 Hz LZeq 60 min on an empty meter). **−** removes a row; **Insert**
  / **Delete** do the same. A 20 Hz LZeq 1 min window at 80 dB: **+**, Tab, ← to 1 min, Tab
  Tab, type 80.
- **+ range…** (**Shift+Insert**): **from ‹20 Hz› to ‹200 Hz› ‹LZeq 60 min› ‹Z›**; **Add**
  (Enter) adds one window per band not yet a window of that kind (× or Delete closes it).
- The §13 **impulse** (+5 / +10 dB) and **narrowband** (+3 / +6 dB) corrections, put in
  force while the character is heard (ac2 does not detect it).

Enter applies. In the terminal:

| Command | What it does |
|---|---|
| `ac2 spl bands set --preset finland-545-lf` | a preset: windows, limits and the predicted window |
| `ac2 spl bands set --windows z:1min@20hz,z:60min@63hz` | the windows, each its length and weighting (Z unless `a:` or `c:`) and its band after `@`; a window kept (same band, length and weighting) keeps its limit; one twice is refused |
| `ac2 spl bands set --windows z:60min@20hz..200hz` | a range is shorthand for one window per band (here eleven) |
| `ac2 spl bands set --limit z:60min@63hz=42db` | a window's limit (`=none` removes it; repeatable; a range sets each band's window) |
| `ac2 spl bands set --day-offset z:60min@20hz..200hz=5db` | the limits are the night's, the day's 5 dB higher (`=none`: the same day and night) |
| `ac2 spl bands set --warn 3db --impulse 5 --tonal none` | every window's warn margin; the §13 corrections |
| `ac2 spl bands set --off` | turns it off |

**Without a transfer** the limits are judged at the mic as typed: right for a meter at the
place itself, or for FOH limits. With one, the place's limits are moved to the mic band by
band. The settings say which ("no band transfer: limits judged at the mic as typed", or the
transfer's summary).

#### What the artist sees

**G** in the SPL pane steps on from meter + Leq to the **bands** (with a band meter; palette
*SPL pane: the band Leq bars*).

- Windows of one length, weighting and day/night scheme share a row of bars, low band to
  high, captioned ("LZeq 60 min · night limits (22–07)"). Each bar is the band's Leq at the
  mic with its limit line: green, amber near, red over; with its headroom ("≤ 81.2") or, over,
  the time until back under.
- The headline names the worst band window and what to do: **"63 Hz band LZeq 60 min 3.2 dB
  over its limit · cooling down in 6 min 52 s"**, or "… 1.5 dB under its limit · next 1 min:
  stay ≤ 81.2 dB".
- Under it: where the limits come from ("limits moved from flat 4 bedroom through the band
  transfer", or estimated), a correction in force, and **"predicted LAeq 60 min in flat 4
  bedroom 23.6 dB · at most 26.1 dB · limit 25.0 dB · NEAR"** ("at most" counts bound bands
  at their bound).
- Uncalibrated, bands read dBFS and nothing is judged. Going over or back toasts like a
  window.

`ac2 spl bands watch` (`--json`: one line a second with a `windows` array, each with
`band_hz`, the `rows` as drawn, the worst window and every text) shows the same.

An A or C band window weights each band at its centre frequency: exact for Z, within a few
tenths of a dB for music; a tone at a band edge can read up to 3 dB off in an A window at
20 Hz ([band-leq.md](design/band-leq.md), *Weighting a band*).

> **Not legal advice, not type-approved**: the presets are informational; the band filters
> meet IEC 61260-1 class 1 but the mic, its calibration and the interface are yours; a
> prediction from FOH is not a measurement at the place. Only a measurement there shows
> compliance.

#### Measuring the band transfer

The **band transfer** is, per band, how much quieter the place is than FOH (its
**attenuation**). Measure it at setup, and again after moved speakers, changed system EQ, an
open window or another room. It needs:

- **the same steady test signal at the same level** at FOH and at the place: pink noise from
  the system, well over the place's background in every band. The two need **not** be
  simultaneous, nor the clocks agree: the signal is steady, so a span of each is enough;
- **the background**: the place with the system silent (without it every band is "no
  background", unchecked);
- **spans of SPL meters' band logs**, each from a calibrated meter with its band meter on,
  within the last 48 h (a start and end on that meter's clock), or a file of `<Hz> <dB>`
  lines.

Per band the result is **clean** (≥ 10 dB over the background), **background subtracted**
(3 … 10 dB over), a **bound** (< 3 dB over: the place is at least that much quieter) or **not
measured** (no limit at the mic). The SPL / Leq settings show it per band.

| Situation | Recipe |
|---|---|
| one mic, carried from FOH to the place (most common) | [1. One mic moved](#1-one-mic-moved) |
| a second mic on the same interface, cabled to the place | [2. Two mics on one interface](#2-two-mics-on-one-interface) |
| the place's mic on another rig (e.g. a Pi there) | [3. The place on another rig](#3-the-place-on-another-rig) |
| no cable to the place, a calibrated recorder | [4. A recorder at the place](#4-a-recorder-at-the-place) |
| no access to the place at all | [5. No access: an estimate](#5-no-access-an-estimate) |

##### 1. One mic moved

All three spans are on the **same meter at different times**, at the same test level:

1. Shift+L, ↑ to a band row, **T**: the band transfer step opens on this meter (its band
   meter on and applied first).
2. Play the test signal, mic at FOH. **Space** starts the FOH span, **Space** stops it (or
   **1–9**: the last 1–9 minutes). Its per-band average reads back, with how many seconds
   were logged.
3. Name the place on the **Limits are for** row (↑, type). Move the mic there, same level:
   **Space**, **Space** for the place's span.
4. Silence the system, mic still there: **Space**, **Space** for the background (or skip:
   every band unchecked).
5. **Enter** computes the transfer and stores it in the meter; **Esc** goes back.

Keys in the step: **↑/↓** place and spans, **Space** start / stop, **1–9** last minutes,
**←/→** the meter of the place's or background span, **Delete** clears a span, **Enter**
stores, **Esc** back. Spans of one meter must not overlap (one mic cannot be in two places).
In the terminal:

```
ac2 spl bands transfer --foh "FOH SPL@21:00:00..21:02:00" \
    --place-levels "FOH SPL@21:05:00..21:07:00" --background "FOH SPL@21:09:00..21:10:00" \
    --place "flat 4 bedroom"
```

(a meter by name or id and a span of its band log: local times today,
`2026-10-08T21:00:00` local, `…Z` UTC, `-30s` before now, `now`).

##### 2. Two mics on one interface

A second calibrated SPL meter on the second input, mic at the place, band meter on. The FOH
and place spans can be the **same** span on the two meters (in the step, **←/→** on the
place's span picks the second meter):

```
ac2 spl bands transfer --foh "FOH SPL@21:00:00..21:02:00" \
    --place-levels "Flat 4@21:00:00..21:02:00" --background "Flat 4@21:04:00..21:05:00" \
    --place "flat 4 bedroom"
```

##### 3. The place on another rig

The place's meter runs on another daemon (a Pi there), calibrated, band meter on. Its spans
are written as `<Hz> <dB>` files and carried over:

```
# on the rig at the place
ac2 spl bands log --from 21:05:00 --until 21:07:00 --levels-out place.txt
ac2 spl bands log --from 21:09:00 --until 21:10:00 --levels-out background.txt
# at FOH
ac2 spl bands transfer --foh "FOH SPL@21:00:00..21:02:00" \
    --place-levels place.txt --background background.txt --place "flat 4 bedroom"
```

`ac2 spl bands log` alone prints the span's per-band average and how much was logged;
`--step N` adds every N-th second (at most 3600 rows).

##### 4. A recorder at the place

1. Loop pink noise from a source that stays put (the console, a player — not the measuring
   laptop) at the show's test level.
2. At the place, record the **calibrator tone**, then the **pink-noise period**, then the
   **background** with the system silent; note roughly when each starts.
3. At FOH, measure the FOH span with the same noise playing (recipe 1, step 2).
4. Import the WAV and replay it **in real time** (`--fast` compresses the clock, so spans
   cannot be named):

   ```
   ac2 rec import ZOOM0001.WAV --name flat4        # writes flat4.wav + flat4.ac2rec.json
   ac2 session replay /path/to/flat4.ac2rec.json
   ```

   File second *t* is the replay's start + *t* on the meter's clock.
5. An SPL meter on the replayed input with its band meter on; while the tone plays,
   calibrate from it (Calibration page, or `ac2 cal spl --input 1 --ref 94db`).
6. Mark the place's and background spans of that meter as the file plays (Space / Space in
   the step), or write them with `ac2 spl bands log --levels-out` and use them as recipe 3.

##### 5. No access: an estimate

If the place cannot be measured at all, type an **estimated** attenuation per band (a file
of `<Hz> <dB>` lines, ≥ 0 dB; a band left out has no limit at FOH):

```
ac2 spl bands estimate --attenuation guess.txt --place "flat 4 bedroom"
```

It is stored as an estimated transfer, shown everywhere as "estimated … typed, not
measured". Measure it as soon as the place can be reached.

### OSC cues

The rig can tell other gear when an SPL limit's state changes: a red light the performers
see, a stage display, a lighting desk. Each limit of an SPL meter (a Leq window, a peak
limit, a band window, the predicted level) can carry a cue, and so can the meter as a whole;
a cue says what each state does, any of them left out: send an OSC message, hold Art-Net
levels ([Art-Net cues](#art-net-cues)), or both:

| state | when |
|---|---|
| `ok` | under the limit, or no limit set |
| `near` | within the warn margin below the limit |
| `over` | over the limit |
| `fault` | the rig cannot judge: no session open, the device lost, the meter stopped, or the limit set on an uncalibrated input |

The messages go over UDP to the rig's OSC target, an IP address and port (no host names: a
name lookup could stall the meter). The daemon keeps the target in its rig settings, so it
survives restarts and no client has to stay connected; send a test message to check the
receiver before the show. A message may carry the measured level (dB SPL) as its last float
argument (`--append-level`; the **+ dB SPL** toggle of a cue row on the SPL / Leq page): every
OSC message of that cue gets it except fault's, which has no level to send; DMX levels never
do. On the SPL / Leq page and the Rig monitor page a typed line is edited in place: the
caret starts at its end, **←/→** and **Home/End** move it (or a click), **Backspace** and
**Delete** remove at it; **Tab** moves to the next cell, **↑/↓** to the next row.

The meter's own cue shows the worst of its limits, `over` first, then `fault`, `near`, `ok`:
an overload always lights, even while another limit cannot be judged. That is the cue for a
single red light.

A change is sent at once, and every cue's current message is sent again every second. A
deleted measurement or a removed cue sends its `ok` once, and so does a clean daemon stop,
so a light is never left red by a meter that no longer exists. Editing a cue does not
restart the window: its level and state carry on.

**Tip:** have the receiver treat more than 3 s without a message as a fault (a watchdog). The
repeats make silence meaningful: a crashed daemon, a pulled network cable or a powered-off
rig then shows as a fault instead of a frozen last state.

From the terminal (`ac2 cue --help` has the whole syntax):

```
ac2 osc target 192.168.1.50:9000          # `off`: send nothing
ac2 osc test /light/red i:1               # one message now, to check the receiver
ac2 cue Stage window:15min --near "/light/amber i:1" --over "/light/red i:1" \
    --ok "/light/off" --fault "/light/blink i:1" --append-level
ac2 cue Stage meter --over "/stage/display s:OVER"
ac2 cue Stage peak:lcpeak --over "/light/strobe 1"
ac2 cue Stage band:63hz:60min --near "/light/amber 2"
ac2 cue Stage window:15min                # show the cue
ac2 cue Stage window:15min --near off     # no message in that state
ac2 cue Stage meter --clear               # remove the cue
ac2 rig status
```

The subject is `meter`, `window:<N>` (the Nth Leq window) or `window:<window>`
(`window:15min`, `window:c:30s`), `peak:lcpeak` or `peak:lafmax`, `band:<hz>:<window>` (Z
unless a weighting is given: `band:63hz:a:15min`) or `predicted`. A message is the address
and its arguments in one quoted argument: `i:1` an integer, `f:0.5` a float, `s:text` a
string; a bare number is an integer without a decimal point and a float with one, any other
bare word a string. States not given keep their message. Another client editing the meter at
the same moment makes the CLI read it again and retry once. In the app, the SPL / Leq page or the
Rig monitor target whose Apply another client's edit beat comes back with your edits and the
line "changed elsewhere: Apply again overwrites with your edits"; Apply (Enter) again sends
them on the newer settings.

#### Art-Net cues

A cue state can also hold DMX levels in one Art-Net universe that ac2 owns: a lamp, an LED
bar or a relay on a DMX node lights straight from the meter, with no OSC receiver in between.
Each state lists channels and levels (`12=255 13=0 14=40`: channel 1–512 = level 0–255). The
universe is the highest level any cue's current state gives a channel (HTP, as lighting
desks merge), and 0 for every channel no current state lists. A cue that goes away stops
holding its levels; a clean daemon stop sends one last frame of every cue's `ok` levels.

ac2 sends all 512 channels whenever the frame changes, and the same frame again every second
(nodes give up on a source silent for a few seconds). A change fades: every channel moves
from its level to the new one over the rig's fade time (1 s until you set another, 0–60 s;
0 jumps), evenly as the eye sees brightness rather than evenly in DMX steps, so a lamp does
not rush through its dark end. A state change during a fade starts a new fade from where
the lights are. Test frames, the cue frame coming back after a test and the last frame of
a clean stop jump. Set the target to a node's address or
to a broadcast address (`2.255.255.255`, or your LAN's), with the universe as a number
0–32767 or as net.sub.uni (`0.0.1`). The levels of a channel no cue sets are 0, so keep ac2's
universe apart from the desk's and merge the two downstream:

- **QLC+**: add an Art-Net *input* on ac2's universe (Inputs/Outputs, the network interface,
  the universe number), and patch the cue channels to the fixtures there; QLC+ merges it
  with its own output.
- **A node with HTP merge** (most Art-Net nodes take two sources per port): send the desk and
  ac2 to the same port-address; the brighter of the two wins on each channel.

```
ac2 artnet target 2.255.255.255 0.0.1     # broadcast, universe 1; `off`: send nothing
ac2 artnet target 192.168.1.40:6454 17    # one node, port given
ac2 artnet fade 1.5                       # cue changes fade over 1.5 s; 0: they jump
ac2 artnet test 12=255 13=40              # light these channels for 5 s to find a fixture
ac2 cue Stage meter --over-dmx "12=255 13=0" --ok-dmx "12=0 13=255" --fault-dmx "14=255"
ac2 cue Stage window:15min --near-dmx "12=80"
ac2 cue Stage meter --fault-dmx off       # no levels in that state
ac2 rig status                            # target, fade, frames sent, the channels above 0
```

The target and the fade are kept in the rig settings like the OSC target. `ac2 artnet test` holds its
frame for 5 s, or until a cue changes state, then the cues' frame comes back.

In the app, Settings › Rig monitor has the same controls under the OSC ones: **Art-Net
target** (`ip[:port]`, IPv4; empty: off) and **Universe** (`N` or `net.sub.uni`), applied
together by Enter on either row; **Fade (s)** (seconds, 0–60; 0 or empty: jump), which Enter
applies; **Send DMX test** (`1=255` to start), which Enter sends;
and a status line with the target, the frames sent, the last error and the channels above 0.
On the SPL / Leq page each cue row has a DMX line under its OSC line, one field per state
(`12=255 13=0`); a field that does not read keeps Enter from applying and says which.

## Keyboard

Everything in the app is reachable from the keyboard.

- **H** (or **F1**) shows the keys that work now: the focused pane's first, then the global
  ones grouped (stimulus; measurements, traces and slots; zoom, axes and cursor; panes and
  window; dialogs and app). The full table is the [keyboard map](#keyboard-map).
- **Ctrl+K** opens the command palette: every command by name, with its key. Typing finds
  a command by its title, its `keys.toml` name or the words a terse title leaves out
  (`unwrap` finds *Phase view*, `grid` *Plot chrome*, `polarity` *Invert polarity*); those
  words are matched, never shown.
- Choices made once and left (key hints, Leq layout and history strip, spectrograph
  history, sweep dB / %, coherence placement) have no key: they are rows of Settings ›
  Display and palette commands.
- Keys are scoped: the focused pane's keys apply first, then the global ones.
- Settings pages have letter keys of their own, listed under each page (Audio I/O: **R**,
  **M**, **S**, **N**, **D**, **P** the driver's settings; Calibration: **N**, **I**, **R**,
  **C**, **E**).
- The defaults avoid `[ ] + - =` and other keys that need AltGr or a dead key on Nordic and
  other European layouts.

The stimulus cluster is fixed: **Space** arm, **Enter** fire, **Esc** stop, **↑/↓** level
±1 dB (Shift: ±3 dB). **Alt+↑/↓** move the selected curve's display offset instead,
**Ctrl+↑/↓** pan the level axis.

**An open window owns the keyboard.** With the help, the palette, a prompt, a dialog or
Settings open: **↑/↓** move or scroll (**PageUp / PageDown**, **Home / End** in long lists);
**←/→** change a choice; **Enter** confirms; **Esc** closes the topmost window only (the
electrical calibration dialog closes back to the Calibration page); **Backspace** erases
typed text and deletes nothing behind the window (where nothing is typed, as in Settings'
lists or a delete confirmation, it acts as Delete). None of these reaches the stimulus, and
the wheel scrolls the window rather than zooming the plot. The help and the delay candidates
leave the other keys working, except the stimulus's.

> **Shift+Esc** stops the stimulus from anywhere, windows included; it is fixed and cannot
> be rebound.

Change bindings in `keys.toml` in the ac2 config directory (`~/.config/ac2` on Linux,
`~/Library/Application Support/ac2` on macOS, `%APPDATA%\ac2\config` on Windows), using the
names in the last column of the keyboard map:

```toml
[global]
cycle_theme = "Alt+T"

[transfer]
insert_delay = ["X", "Alt+D"]
```

The sections follow the keyboard map: `[global]` (Everywhere), `[transfer]`, `[spectrum]`,
`[ir]` (the impulse response view), `[spl]` and `[distortion]` (Sweep / distortion).

A `keys.toml` that cannot be used (a conflict, an unknown key or command, a pane command on
**H**, a change to **Shift+Esc**) is reported at start and the defaults are used until it is
fixed.

#### Key hints

The focused pane shows a slim line under its plot with its most used keys (each pane's line
is listed at the end of the [keyboard map](#keyboard-map)). It shows the keys bound now
(macOS: `⌘ ⌥ ⇧`); on a narrow pane the least used drop first, **H all keys** stays. Hovering
over the line lists the keys with what each does; hovering over a clickable control (the
**dB | %** toggle, **Stop**, a colour dot or row, a pane's chip) names its key. Settings ›
Display › Key hints (palette *Key hints*) turns it off (`key_hints = false` in
`ui.toml`). The stage view never shows it.

### Notifications

What a key or reply did, and what went wrong, shows in the bottom-right corner above the key
hints. A box is as wide as its text (up to about 45 % of the window, then wrapped); newer
ones sit lower and push older ones up; a repeated message replaces the one already up.

| colour | what | stays |
|---|---|---|
| panel colour | information: what a key or a reply did | 3 s + 0.3 s per word, at most 15 s |
| warning colour (as the banners) | a key refused or something missing, with what to do instead | 1.5 × that, at most 25 s |
| fault colour (as the banners) | a command, the link or the stimulus failed; an Leq limit went over | 3 × that, 20–60 s |

The pointer resting on them holds them all; a click dismisses one. Esc does not (it belongs
to the open window or the stimulus).

**Warning toasts** (Settings › Display, or palette *Warning toasts on / off*; on by default,
`warning_toasts = false` in `ui.toml`) off keeps warnings and Leq limit alarms out of the
corner while you go over a limit on purpose (testing a limiter); they still go to
*Notifications…* and the SPL displays still turn the limit colour.

**Notifications…** (**Shift+E**, or the palette, type `notif`) lists the app's last 50, newest first, with
kind, age and repeat count; Esc or Enter closes it. The log lasts as long as the app runs.

### Keyboard map

<!-- keymap:begin (generated by crates/ac2-ui/tests/it/keymap_doc.rs) -->
Keys as on Linux and Windows; on macOS `Ctrl` is `⌘` and `Alt` is `⌥`. Every binding can be changed in `keys.toml` using the names in the last column.

#### Everywhere

| Keys | Command | `keys.toml` |
|---|---|---|
| `H` or `F1` | Key bindings | `help` |
| `Shift+E` | Notifications… | `notifications` |
| `Ctrl+K` | Command palette | `palette` |
| `Ctrl+P` | Settings… | `settings` |
| `Ctrl+Q` | Quit | `quit` |
| `F` or `F11` | Full screen | `fullscreen` |
| `Space` | Arm stimulus | `stimulus_arm` |
| `Enter` | Fire stimulus | `stimulus_fire` |
| `Esc` | Stop stimulus | `stimulus_stop` |
| `Shift+Esc` | Stop stimulus anywhere | `stimulus_stop_anywhere` |
| `↑` | Stimulus level +1 dB | `level_up` |
| `↓` | Stimulus level −1 dB | `level_down` |
| `Shift+↑` | Stimulus level +3 dB | `level_up_coarse` |
| `Shift+↓` | Stimulus level −3 dB | `level_down_coarse` |
| `L` | Set stimulus level… | `stimulus_level` |
| `Alt+1` | Focus pane 1 | `focus_pane_1` |
| `Alt+2` | Focus pane 2 | `focus_pane_2` |
| `Alt+3` | Focus pane 3 | `focus_pane_3` |
| `Alt+4` | Focus pane 4 | `focus_pane_4` |
| `Alt+5` | Focus pane 5 | `focus_pane_5` |
| `Alt+6` | Focus pane 6 | `focus_pane_6` |
| `Alt+7` | Focus pane 7 | `focus_pane_7` |
| `Alt+8` | Focus pane 8 | `focus_pane_8` |
| `Alt+9` | Focus pane 9 | `focus_pane_9` |
| `N` | Split pane | `split_pane` |
| `Q` | Close pane | `close_pane` |
| `Shift+N` | Turn split | `turn_split` |
| `Alt+→` | Grow pane | `grow_pane` |
| `Alt+←` | Shrink pane | `shrink_pane` |
| `W` | Maximise pane | `maximise_pane` |
| `Ctrl+Z` | Undo pane change | `undo` |
| `Ctrl+Shift+Z` | Redo pane change | `redo` |
| `Tab` | Next list row | `next_in_tree` |
| `Shift+Tab` | Previous list row | `prev_in_tree` |
| `T` | Plot chrome | `plot_chrome` |
| `I` | Zoom in | `zoom_in` |
| `O` | Zoom out | `zoom_out` |
| `←` | Pan left | `pan_left` |
| `→` | Pan right | `pan_right` |
| `Home` | Reset zoom | `reset_view` |
| `Ctrl+I` | Zoom level in | `level_zoom_in` |
| `Ctrl+O` | Zoom level out | `level_zoom_out` |
| `Ctrl+↑` | Pan level up | `level_pan_up` |
| `Ctrl+↓` | Pan level down | `level_pan_down` |
| `Shift+Home` | Fit level | `level_fit` |
| `Ctrl+Home` | Reset axes | `level_reset` |
| `Shift+T` | Comparison cursor | `toggle_cursor` |
| `C` | Compare… | `compare` |
| `Shift+←` | Cursor left | `cursor_left` |
| `Shift+→` | Cursor right | `cursor_right` |
| `Ctrl+1` | Capture to slot 1 | `slot_1` |
| `Ctrl+2` | Capture to slot 2 | `slot_2` |
| `Ctrl+3` | Capture to slot 3 | `slot_3` |
| `Ctrl+4` | Capture to slot 4 | `slot_4` |
| `Ctrl+5` | Capture to slot 5 | `slot_5` |
| `Ctrl+6` | Capture to slot 6 | `slot_6` |
| `Ctrl+7` | Capture to slot 7 | `slot_7` |
| `Ctrl+8` | Capture to slot 8 | `slot_8` |
| `Ctrl+9` | Capture to slot 9 | `slot_9` |
| `1` | Show/hide slot 1 | `show_slot_1` |
| `2` | Show/hide slot 2 | `show_slot_2` |
| `3` | Show/hide slot 3 | `show_slot_3` |
| `4` | Show/hide slot 4 | `show_slot_4` |
| `5` | Show/hide slot 5 | `show_slot_5` |
| `6` | Show/hide slot 6 | `show_slot_6` |
| `7` | Show/hide slot 7 | `show_slot_7` |
| `8` | Show/hide slot 8 | `show_slot_8` |
| `9` | Show/hide slot 9 | `show_slot_9` |
| `V` | Next trace | `next_trace` |
| `Shift+V` | Previous trace | `prev_trace` |
| `Alt+V` | Next trace incl. hidden | `next_any_trace` |
| `Alt+Shift+V` | Previous trace incl. hidden | `prev_any_trace` |
| `A` | Show/hide curve | `toggle_selected` |
| `F2` | Rename trace… | `trace_rename` |
| `End` | Select live | `select_live` |
| `Delete` or `Backspace` | Delete selected… | `delete_selected` |
| `Alt+↑` | Offset +1 dB | `offset_up` |
| `Alt+↓` | Offset −1 dB | `offset_down` |
| `Alt+Shift+↑` | Offset +3 dB | `offset_up_coarse` |
| `Alt+Shift+↓` | Offset −3 dB | `offset_down_coarse` |
| `Alt+Home` | Clear offset | `offset_clear` |
| `Alt+S` | Sessions… | `sessions` |
| `Ctrl+S` | Save session | `session_save` |
| `Shift+O` | Open audio I/O… | `session_open` |
| `Ctrl+M` | New measurement… | `meas_new` |
| `Shift+M` | New math… | `meas_new_math` |
| `Alt+M` | New alignment… | `meas_new_alignment` |
| `Shift+C` | Next colour | `meas_colour_next` |
| `Shift+A` | Show/hide measurement | `hide_group` |
| `Shift+F2` | Move trace… | `move_trace` |
| `Shift+S` | New sweep… | `sweep_new` |
| `Shift+L` | Leq settings… | `leq_windows` |

#### Transfer function

| Keys | Command | `keys.toml` |
|---|---|---|
| `R` | Reset averaging | `reset_average` |
| `S` | Start/stop | `start_stop` |
| `X` | Find first arrival | `insert_delay` |
| `Shift+X` | Find strongest peak | `insert_strongest` |
| `D` | Set delay… | `type_delay` |
| `Y` | Delay tracking | `track_delay` |
| `Ctrl+,` | Delay −1 sample | `delay_down` |
| `Ctrl+.` | Delay +1 sample | `delay_up` |
| `Alt+,` | Delay −0.1 sample | `delay_down_fine` |
| `Alt+.` | Delay +0.1 sample | `delay_up_fine` |
| `U` | Invert polarity | `invert` |
| `J` | Set offset… | `offset` |
| `,` | Delay −0.1 ms | `nudge_earlier` |
| `.` | Delay +0.1 ms | `nudge_later` |
| `E` | Set phase reference | `phase_reference` |
| `Z` | Import target… | `target` |
| `B` | Coherence mask | `coherence_mask` |
| `M` | Average power | `average` |
| `P` | Min/max tracking | `min_max` |
| `Shift+P` | Phase view | `phase_view` |
| `K` | Smoothing coarser | `smooth_coarser` |
| `Shift+K` | Smoothing finer | `smooth_finer` |
| `G` | Next view | `next_view` |
| `Shift+G` | Previous view | `prev_view` |

#### Spectrum / RTA

| Keys | Command | `keys.toml` |
|---|---|---|
| `R` | Reset averaging | `reset_average` |
| `S` | Start/stop | `start_stop` |
| `J` | Set offset… | `offset` |
| `K` | Smoothing coarser | `smooth_coarser` |
| `Shift+K` | Smoothing finer | `smooth_finer` |
| `B` | Spectrum style | `spectrum_style` |
| `P` | Peaks | `peak_hold` |
| `Shift+B` | Spectrograph span | `spectrograph_span` |
| `G` | Next view | `next_view` |
| `Shift+G` | Previous view | `prev_view` |

#### Transfer: impulse response view

| Keys | Command | `keys.toml` |
|---|---|---|
| `S` | Start/stop | `start_stop` |
| `G` | Next view | `next_view` |
| `Shift+G` | Previous view | `prev_view` |

#### SPL

| Keys | Command | `keys.toml` |
|---|---|---|
| `R` | Reset averaging | `reset_average` |
| `S` | Start/stop | `start_stop` |
| `G` | Next view | `next_view` |
| `Shift+G` | Previous view | `prev_view` |
| `Shift+R` | New SPL log… | `spl_new_log` |
| `Shift+F` | Time weighting | `spl_time_weighting` |
| `Z` | Frequency weighting | `spl_weighting` |

#### Sweep / distortion

| Keys | Command | `keys.toml` |
|---|---|---|
| `G` | Next view | `next_view` |
| `Shift+G` | Previous view | `prev_view` |
| `Shift+I` | Sweep IR view | `sweep_ir` |
| `E` | Reference harmonics | `reference_harmonics` |

#### Command palette only (`Ctrl+K`)

| Command | `keys.toml` |
|---|---|
| Key hints | `key_hints` |
| Warning toasts | `warning_toasts` |
| Stimulus outputs… | `stimulus_outputs` |
| Take over stimulus | `stimulus_take_over` |
| Transfer stack | `transfer_stack` |
| Choose pane measurement… | `pane_measurement` |
| Theme | `cycle_theme` |
| Clear compare | `clear_compare` |
| Move trace to slot… | `trace_slot` |
| Export trace… | `trace_export` |
| Export pane as PNG… | `pane_export_png` |
| Import trace… | `import_trace` |
| New session… | `session_new` |
| Open session… | `session_load` |
| Save session as… | `session_save_as` |
| Export session… | `session_export` |
| Reconnect | `reconnect` |
| Close audio I/O | `session_close` |
| Record | `record` |
| Replay recording… | `replay_recording` |
| New transfer… | `meas_new_transfer` |
| New spectrum… | `meas_new_spectrum` |
| New RTA… | `meas_new_rta` |
| New SPL meter… | `meas_new_spl` |
| Edit measurement… | `meas_edit` |
| Previous colour | `meas_colour_prev` |
| Fold/unfold measurement | `toggle_group` |
| Input setup… | `input_setup` |
| Calibrations… | `calibrations` |
| Set input mics… | `input_mics` |
| Next mic curve | `mic_curve` |
| Set input mic curve… | `mic_curve_input` |
| Delete calibration… | `cal_delete` |
| Set trace mic curve… | `trace_mic_curve` |
| Fast time weighting | `spl_fast` |
| Slow time weighting | `spl_slow` |
| Impulse time weighting | `spl_impulse` |
| A weighting | `spl_a` |
| C weighting | `spl_c` |
| Z weighting | `spl_z` |
| Finder band auto | `finder_auto` |
| Finder band full | `finder_full` |
| Finder band mid | `finder_mid` |
| Finder band sub | `finder_sub` |
| Finder band custom… | `finder_custom` |
| Finder observation… | `finder_observation` |
| Coherence placement | `coherence_placement` |
| Legend top-left | `legend_top_left` |
| Legend top-right | `legend_top_right` |
| Legend bottom-left | `legend_bottom_left` |
| Legend bottom-right | `legend_bottom_right` |
| Show/hide legend | `legend_hide` |
| Average complex | `average_complex` |
| Average coherence-weighted | `average_coherence` |
| Smoothing off | `smooth_off` |
| Smoothing 1/48 oct | `smooth_48` |
| Smoothing 1/24 oct | `smooth_24` |
| Smoothing 1/12 oct | `smooth_12` |
| Smoothing 1/6 oct | `smooth_6` |
| Smoothing 1/3 oct | `smooth_3` |
| Next colour map | `spectrograph_colours` |
| Colour map viridis | `spectrograph_colours_viridis` |
| Colour map magma | `spectrograph_colours_magma` |
| Colour map inferno | `spectrograph_colours_inferno` |
| Colour map cividis | `spectrograph_colours_cividis` |
| Colour map grey | `spectrograph_colours_grey` |
| Meter view | `spl_show_meter` |
| Leq view | `spl_show_leq` |
| Meter + Leq view | `spl_show_meter_leq` |
| Band Leq view | `spl_show_bands` |
| Leq style | `spl_leq_style` |
| Leq history | `spl_leq_history` |
| Export Leq log… | `spl_log_export` |
| Export previous Leq log… | `spl_log_export_previous` |
| Distortion unit | `distortion_unit` |

#### Key hint lines (Settings › Display › Key hints)

The least used go first on a narrow pane; `G/Shift+G` steps the pane's views forward and back in every pane.

| Pane | Hint line |
|---|---|
| Transfer function | `V` select trace · `G/Shift+G` view · `A` show/hide · `Ctrl+1` capture · `X` find delay · `B` coherence mask · `P` min/max · `K` smoothing · `H` all keys |
| Spectrum / RTA | `S` start/stop · `P` peaks · `G/Shift+G` view · `K` smoothing · `Shift+Home` fit level · `Ctrl+1` capture · `W` maximise · `H` all keys |
| Transfer: impulse response view | `G/Shift+G` view · `I` zoom time · `Ctrl+I` zoom level · `Shift+Home` fit level · `Tab` next row · `Q` close pane · `W` maximise · `H` all keys |
| SPL | `G/Shift+G` view · `Shift+F` F/S/I · `Z` A/C/Z · `Shift+L` windows · `Shift+R` new log · `W` maximise · `H` all keys |
| Sweep / distortion | `Shift+S` new sweep · `V` select trace · `G/Shift+G` view · `E` reference · `W` maximise · `Q` close pane · `H` all keys |

<!-- keymap:end -->

## Command line

Every command takes `--json` for machine-readable output. Commands that talk to the daemon
take `--remote <host[:port]>` for a network daemon, `--timeout` (the reply deadline per
try, default `1.5s`) and `--key-dir` (the client key pair and pinned daemon keys). Live views (`--watch`) redraw in the terminal. Units are written with
suffixes: `-20dbfs`, `48khz`, `12.5ms`, `600samples`, `4.3m`, `94db`. `ac2 <command> --help`
documents each command.

| command | what it does |
|---|---|
| `ac2 devices`, `ac2 status`, `ac2 daemon start / stop / status` | the daemon and its audio devices; `status` includes the autosave state |
| `ac2 session open / close / status / inputs / replay` | the audio I/O (`open --out-device` plays on another device than the capture one), each input's mic and active curve; replaying a recording as the audio input |
| `ac2 session panel [--backend B] [--device D]` | opens the device's driver settings on the daemon's host (an ASIO driver's control panel, Audio MIDI Setup for Core Audio) |
| `ac2 session new / save / save-as / load / recover / salvage / export / import / list` | the working state and saved sessions ([Sessions](#sessions)) |
| `ac2 meas new / list / set / start / stop / rm`, `ac2 meas layout clear` | transfer (`tf`), `spectrum`, `rta`, `spl` and `sweep` measurements (math channels are listed too); `rm` of one that owns traces needs `--keep-traces` (moved to Imported) or `--delete-traces`; `layout clear` removes a measurement's own panes |
| `ac2 math new / set` | math channels: `"A / B"`, `--op div\|mul\|add\|sub --a A --b B`, `--op avg --of A,B,C` |
| `ac2 gen pink / white / periodic-pink / sine`, `ac2 gen stop` | the generator in the foreground (Enter fires, Esc stops); `stop` from any client |
| `ac2 gen ceiling [LEVEL] [--yes]` | the system max level: shown with its bound and who changed it; a lower level applies at once, a higher one needs `--yes` and nothing armed |
| `ac2 delay find / insert / set / nudge / track` | the delay finder and delay of a transfer measurement |
| `ac2 sweep run <meas>` | a run of a sweep measurement with its settings, stored under it |
| `ac2 ir capture`, `ac2 ir metrics` | a sweep with its settings as flags (runs the sweep measurement with exactly those settings, made when there is none); room parameters of a stored sweep |
| `ac2 rec start / stop / status / list / import` | raw recording of the inputs |
| `ac2 trace capture / list / show / rename / display / slot / move / rm / average / import / export / smooth / mic / delay-diff` | stored traces; `list` says what each is filed under; `move <traces> --to MEAS` (or `--imported`) files them elsewhere |
| `ac2 cal spl / electrical / curve import / curve rename / curve rm / use / list / rm` | sensitivity calibrations and the mic library |
| `ac2 spl watch / set / cal`, `ac2 spl leq watch / set / export / new` | SPL readout, the meter's weightings, acoustic calibration (as `cal spl`), Leq windows and presets, the per-second log and a new log |
| `ac2 spl bands watch / set / transfer / log / estimate` | the band meter and its transfer ([Band Leq](#band-leq-band-limits-at-a-neighbours)) |
| `ac2 timing --watch` | the loopback timing monitor |
| `ac2 state dump` | the daemon's whole state as JSON |
| `ac2 selftest duplex --backend jack\|coreaudio\|asio\|fake` | an interface checked without a daemon (stop one that holds it); the backend has no default, `--device` / `--out-device`, `--in`, `--outputs`, `--rate`, `--buffer` choose the stream. Runs it for `--duration` (default 30 s) and prints pass or fail with named reasons (xruns, gaps, clock rate, …) and the latency the backend states (each way and the round trip, where it states one); silent unless `--emit <level> --loopback-out N --loopback-in N` plays pink noise (at most −20 dBFS) on a loopback cable to time output→input and drift |
| `ac2 discover`, `ac2 auth pair / show` | find network daemons, pair with one |
