# ac2 user guide

ac2 is a live dual-channel FFT analyzer for tuning sound systems: transfer functions with
coherence, a delay finder, sweep measurements with harmonic distortion, spectrum and RTA, a
calibrated SPL meter with rolling Leq windows, stored traces, sessions and autosave. This
guide explains the concepts and the everyday workflow. Installing: [install.md](install.md). Scripting and integrations: [protocol.md](protocol.md), the
normative description of everything the daemon speaks.

## How the pieces fit

```
audio interface ── ac2d (daemon) ──┬── ac2-ui  (the app: plots, keys)
                                   ├── ac2     (CLI: setup, scripting, --json)
                                   └── your own client (ZeroMQ + msgpack, docs/protocol.md)
```

The **daemon** owns the audio interface and does all measuring. The app and the CLI are
clients; several can be connected at once (a laptop at FOH and another at a delay tower),
and they all see the same measurements. Locally the daemon is reachable only by your user;
across the network it requires pairing ([install.md](install.md#remote-use-foh--stage)).

A **session** is the open audio stream: one device, a sample rate, a buffer size and the
inputs to capture. **Measurements** run on the session: transfer functions (`tf`), spectrum,
RTA and SPL meters. In the app, **Shift+O** opens the session dialog
([below](#the-session-dialog)), and the command palette (**Ctrl+K**) has *New transfer
measurement…*, *New spectrum…*, *New RTA…* and *New SPL meter…* (created, started and
selected on **Enter**), *Delete selected measurement* and *Close audio session*. Until there
is a session, or a measurement, the transfer pane says which of these comes next. This
works the same against a daemon the app hosts, a per-user daemon and a remote one. The CLI
does the same from a script, with the same defaults:

```sh
ac2 session open --backend jack --in 1-4                     # Linux: the JACK server
ac2 session open --backend cpal --device "<name>" --in 1-4 --rate 48khz   # macOS, Windows
ac2 meas new tf --ref 1 --meas 2 --name main-l
ac2 meas new tf --ref 1 --meas 3 --name sub
ac2 meas new rta --input 2 --name rta
ac2 meas start main-l
```

The backend is always named. Each platform has one real backend: `jack` on Linux (a JACK2
server, or PipeWire through pipewire-jack; there is no ALSA backend), `cpal` on macOS and
Windows (Core Audio, WASAPI). On JACK the rate and buffer are the server's; on cpal, with
no `--buffer`, ac2 asks for a short fixed buffer of about 20 ms (1024 frames at 48 kHz,
within what the device allows) rather than the host's default, which can be large enough
to deliver audio in lumps. Should a host still deliver in lumps, the daemon log says so
once a minute (*audio arrives in bursts on …*) with the device and buffer to change.
`fake` is a simulated rig for trying things out and is never chosen for you: the session
dialog preselects a real interface whenever the daemon lists one. Choosing
*Simulated rig* in the app's connect dialog starts it ready to measure (session open, a
transfer measurement "demo" running); *This computer's audio* starts with no session and
opens the session dialog.

### The session dialog

Nothing in the dialog is a channel number to type. From the top:

- **Backend** (**←/→**): every backend the daemon offers, with what it is; one it cannot use
  now says why and what to do (*PipeWire is running but its JACK library isn't in use:
  install pipewire-jack … or start the daemon with `pw-jack ac2d`*). A daemon on real
  audio offers its platform's backend (JACK on Linux, the system's audio on macOS and
  Windows); the simulated rig only appears on a daemon started on it.
- **Device** (**←/→**): its name and *N in / M out · rate · buffer*.
- **Channels**: one row per input and output, named by the backend where it can (JACK port
  names; system audio has none, so *Input 3*), and for inputs a **live meter** (RMS bar,
  peak tick, *CLIP* held for a second). The meters run before the session opens: the daemon
  opens the device for capture only — no output stream exists — so you can find the mic by
  tapping it. While a session is open on that device the dialog shows the session's own
  meters instead.
- **Roles**: **R** Reference (the loopback return, one input), **M** measurement mic (any
  number; **N** names one — the name is the mic's identity for calibrations; a named mic's
  row says which curve and calibration it uses, and **←/→** on it choose the curve: off, 0°,
  90° …), **S** Stimulus
  (the output feeding the system and the loopback). **Space** puts a row in or out of the
  session. The mouse works too: the boxes, the R / M / S chips, a double click on a name.
- **Detect loopback…** (**D**): asks for a level (no default; the stimulus level you typed
  last is offered), then on **Enter** plays a 0.5 s band-limited noise burst on the stimulus
  output — under the stimulus lease and the global ceiling, faded in and out — and marks the
  input it returns on as the Reference (or says that none answered and which came closest).
- **Enter** opens; what is missing is said in words (*Pick a reference input: the loopback
  from your stimulus output*). The session's inputs, outputs and loopback follow from the
  roles; the roles and mic names are remembered per device (`ui.toml`) and come back next
  time. With a Reference, at least one mic and no measurements yet, one more **Enter**
  creates *Reference → <mic>* transfer measurements. **Esc** closes the dialog; the
  stimulus is left as it was (**Shift+Esc** stops it).

The measurement dialogs pick inputs the same way: by name, with their meters, **←/→**.

### Input meters

While a session is open, the top of the left-hand list shows **Inputs**: one live meter per
input the session captures — RMS as the bar, the sample peak as a tick, the RMS in dBFS
beside it, and *CLIP* in red, held for a second after the input clipped. The meters run
whether or not anything measures, and keep running during sweeps and other operations, so
the mic and the reference stay in sight while you set levels.

Each row is named, not numbered: the mic's name, else the backend's channel name (the JACK
port), else *Input N*; then its role — *reference* (the loopback input, or the reference of
a transfer measurement) or *mic* (a named mic, or the measured input of a transfer
measurement) — and the input number when the name does not already say it:
*MM1 34804 · 90° · mic (in 1)*, *loopback · reference (in 2)*, *capture_4 (in 4)*. A
named mic's row also says which mic curve is in use — its label (*90°*), or why none is:
*curve off*, *curve not chosen*, *no curve stored*, *90° not stored*. A **REF** /
**MEAS** mark shows what the running sweep uses, or, with nothing running, the selected
measurement.

## Reference wiring and loopback

A transfer function divides what the microphone hears by what you sent. "What you sent" is
the **reference** input: the stimulus, taken electrically from the output that feeds the
system and looped back into an input of the same interface. Both inputs are sampled on one
clock, so the comparison is exact no matter what the output side does.

```
out 1 ──┬──► processor / amp / speaker ··· mic ──► in 2   (measurement)
        └──► loopback cable ─────────────────────► in 1   (reference)
```

ac2 needs that reference: without signal on it, the transfer pane shows **NO REFERENCE**
instead of a curve. With a loopback that also returns the generator's own output (the
**R** and **S** roles in the session dialog, `session open --loopback-out 1 --loopback-in 1`
from the CLI), the daemon continuously checks the output → input timing and warns about
dropped or repeated output samples.

You can measure any signal, not only ac2's generator: program material from the console,
fed to the reference input, works the same way (coherence then tells you which frequencies
the music actually covered).

## Stimulus

The generator plays pink, periodic pink, white noise or a sine. It is deliberately hard to
make it play by accident:

- nothing plays without a typed **level** (`L` in the app, `--level` in the CLI); there is
  no default level;
- **arm** (`Space`) and **fire** (`Enter`) are separate steps; **Esc** stops (with a window
  open, Esc first closes the window), **Shift+Esc** stops from anywhere, windows included —
  either from any client, without needing anything else. While anything is armed or
  playing the top bar shows **■ Stop: Shift+Esc**;
- one client holds the stimulus at a time (a lease it keeps refreshing). If that client
  disappears, the daemon fades the output out within 1.5 s;
- the daemon has a global maximum level (`ac2d --max-level`, default −10 dBFS RMS);
- loading a session or restarting the daemon always comes up disarmed.

## Transfer measurement

The transfer pane shows **magnitude**, **phase** and **coherence** of measurement / reference
on a log-frequency grid (48 points per octave), computed with a multi-time-window FFT ladder
so low frequencies get long windows and high frequencies stay responsive. Coherence (γ²) is
how much of the measured energy is explained by the reference: drawn as the transparency of
the traces, as its own curve, or as a mask that blanks the traces where γ² is below a chosen
threshold (`B` cycles through off, 0.3, 0.5, 0.7, 0.9).

Averaging accumulates the cross- and auto-spectra; **F** freezes the average, **R** resets
it. Smoothing (1/3 … 1/48 octave) is applied after coherence, so it never makes bad data look
coherent.

### Choosing what a pane shows

Each pane's title has a **chip** naming the measurement it shows (the transfer pane draws
every transfer measurement, with that one first in the legend; the IR pane follows the
transfer pane's choice). Click the chip for the list of measurements the pane can show and
pick one, or use the keyboard: focus the pane (click it, **Tab**, **Alt+1 … Alt+4**) and
**N** / **Shift+N** step through that pane's kind only — transfer measurements in the
transfer pane, spectra and RTAs in the spectrum pane. **Choose the measurement the focused
pane shows…** in the palette opens the same list (**↑/↓**, **Enter**).

A click inside a pane selects the measurement it shows, exactly as clicking it in the
measurement list does; selecting one in the list makes its pane show it and gives that pane
the focus (unless the focused pane draws it already: the IR pane keeps it for a transfer
measurement). **S** (start / stop) and **R** (reset) act on the measurement the focused pane
shows — never on one of another kind selected elsewhere — and say what to create when the
pane shows none. **F**, **X** and the delay keys act on the selected measurement and say
which kind they need when it is another.

**One pane only, full screen (W).** **W** steps through three layouts: the split layout →
the focused pane alone in the window → that pane **full screen** → the split layout again.
Full screen is the **stage view**: the window fills the screen and holds the pane's picture
alone — no top bar, measurement list or pane title; a plot keeps a one-line caption naming
the pane and its measurement, the SPL meter and its Leq windows name themselves. Key hints
are off there. Whenever a stimulus is armed or playing, or a sweep or another operation
runs, the top bar comes back: what drives the speakers is never hidden. **F11** on its own
puts the whole window full screen (or back) in whatever layout it is in — with one pane up,
that is the same stage view. Esc stops the stimulus as anywhere else (Shift+Esc too, also
with a window open).

With the focused pane maximised, picking a measurement in the list switches the one pane to
the pane that shows it — a transfer measurement to the transfer pane, a spectrum or RTA to
the spectrum / RTA pane, an SPL meter to the SPL pane — and the layout stays maximised. A
stored trace selected while maximised (a click in the Traces list, **V**) does the same: a
transfer capture or target brings up the transfer pane, a spectrum capture the spectrum
pane, a sweep the sweep pane (unless the transfer pane is up: it draws sweeps too). In the
split layout every pane is on screen and selecting a trace leaves the focus where it is.

**The layout comes back.** The app remembers in `ui.toml` (written when the layout changes
and on exit) which pane has the focus, whether it is maximised or full screen, the
measurement each pane shows (by name), the SPL pane's view (meter, Leq windows or both), the Leq windows'
style, the IR mode, the sweep pane's dB / % and the window's size and position, and each
pane's level axis range (see below). The next
start comes back to them — full screen too — without arming or playing anything; a
measurement that is gone (deleted, another daemon) quietly leaves its pane on its usual
choice, and a window larger than the screen it opens on is made to fit. On Wayland the
system places the window.

### Zoom, pan and the level axis

The frequency axis is shared by the panes: **I** / **O** zoom it, **←** / **→** pan it,
**Home** resets it to 20 Hz – 20 kHz; the mouse wheel zooms about the pointer and a drag
pans.

Each pane's **level axis** (transfer magnitude, spectrum / RTA level, the sweep pane's
distortion) is its own, and the app remembers it in `ui.toml` for the next start (a fit made
for one show is a fair start for the next; **Ctrl+Home** forgets it). A spectrum that starts
still fits its level axis on its first frame:

- **Ctrl+I** / **Ctrl+O** zoom the focused pane's level axis in / out about its middle;
  **Ctrl+wheel** zooms it about the level under the pointer.
- **Ctrl+↑** / **Ctrl+↓** pan it by a round step (about a tenth of the range: 10 dB of a
  100 dB range); **Shift+wheel** pans it smoothly. Plain **↑/↓** stay the stimulus level.
- **Shift+Home** fits it to what the pane shows (live and stored curves, display offsets
  included; a few empty bins far below do not stretch it) and puts the frequency axis back
  to 20 Hz – 20 kHz: the way to look at very low signals, e.g. a spectrum between −140 and
  −80 dBFS. **Ctrl+Home** puts the pane's default ranges back (transfer ±30 dB, spectrum
  0 … −100 dBFS or 20 … 120 dB SPL, 20 Hz – 20 kHz).
- The spectrum pane keeps one level range per scale: **dBFS** for uncalibrated curves and
  **dB SPL** once its input is calibrated, chosen by the scale its curves are shown in, so a
  calibration never leaves the curves above a dBFS-sized axis. Zoom, pan, fit and reset act
  on the range in use.
- The labels follow the range: tenths of a dB on a 1 dB range, tens on a 100 dB one.

**What the spectrum's level means.** A narrowband spectrum's levels are **per FFT bin**,
and its axis says how wide a bin is: `dBFS per 1.46 Hz bin (tone)` (48 kHz / 32 768
points), `dB SPL per 0.73 Hz bin (tone)`. A sine reads its RMS level whatever the FFT length
(*tone* level), but broadband sound — pink noise, programme, a crowd — spreads its power over
many bins, so each bin reads far below the band or total level, and lower the finer the bins:
3 dB lower per doubling of the FFT length. That is why a calibrated spectrum of a loud room
can sit at 40 dB SPL, and an uncalibrated one below −100 dBFS. **Use an RTA for band levels
in dB SPL** (its axis says `(band)`); hovering over the spectrum's unit says the same. The
width named is the bin spacing (sample rate / FFT length); with the Hann window broadband
sound reads 1.8 dB above what that spacing alone would give (the window's noise bandwidth
is 1.5 bins). Curves on different FFT lengths share `per bin, mixed widths`; a narrow pane
shortens the unit (`dBFS per 1.46 Hz bin`, `dBFS/bin`, `dBFS`).

A live **narrowband spectrum** is drawn from display columns, not from every FFT bin: each
bin is its own column while bins are wider than 1/96 octave (up to about 100 Hz at the
default 65 536 points and 48 kHz), above that a column spans 1/96 octave and shows the
highest bin in it. A tone keeps its level, and the cursor reads its frequency to within
1/96 octave; that is one column every pixel or two over 20 Hz – 20 kHz, and keeps the
spectrum small enough for a laptop on WiFi. For finer detail, zoom in on a **capture**
(**Ctrl+1**): a stored spectrum keeps every bin. A long FFT updates every eighth of its
window — about 6 times a second at 65 536 points — since windows overlapping more than that
add work but no new information; short FFTs update about 30 times a second.

### Spectrograph

**G** in the spectrum pane shows the **spectrograph** of the pane's measurement under the
spectrum: frequency across on the spectrum's own axis (zoom and pan move both), time down with
the newest frame at the top, level as colour. The colour bar on the right spans the pane's
level axis, so **Ctrl+I / Ctrl+O**, **Ctrl+↑/↓**, **Shift+Home** and **Ctrl+Home** change the
colours as they change the curve's axis; the colours are a perceptual, colour-blind-safe map
(viridis): equal steps in dB look like equal steps. **Shift+G** steps the history through 10,
30 (the default), 60 and 120 s; it starts afresh at each length and when the spectrograph is
switched on.

- A click in the spectrograph puts the cursor there: above the plot it reads frequency, time
  before the newest frame and level (`1.00 kHz · 4.2 s ago · −32.0 dBFS`); **C** turns it
  off. The spectrum's cursor line runs through both.
- Time without frames — the stream went STALE, the measurement was stopped — is a gap (the
  plot's background), never the last spectrum stretched over it. A long FFT that updates a few
  times a second fills the time between its frames with each frame.
- Where a pixel covers several frames or frequencies it shows the highest level among them,
  as the spectrum's line does: a short event or a narrow tone is never lost between pixels.
- A stopped measurement keeps its spectrograph, and the caption says `stopped`; a STALE one is
  dimmed like its curve. Changing the FFT length, band fraction or calibration (dBFS ↔ dB SPL)
  starts the history over.
- The history is kept by the app, from the frames it already receives: nothing extra on the
  wire, and nothing kept while the spectrograph is hidden.

### Smoothing

**K** makes the smoothing coarser and **Shift+K** finer, through off, 1/48, 1/24, 1/12, 1/6
and 1/3 octave; the palette also sets a step directly (**Smoothing: 1/6 oct**, …). The keys
work in the transfer pane and in the spectrum pane. Each pane's title says the smoothing of
what it shows (`smoothing 1/6 oct`), and every transfer legend row shows the smoothing of its
curve.

- **Transfer functions** smooth the magnitude (power-averaged) **and the phase**, so the
  phase pane, unwrapped phase and group delay all read the smoothed curve. The phase is
  unwrapped within each run of valid columns before it is averaged, so a wrap never drags
  the average towards 0°. That needs the delay set first: a residual delay of more than about
  34 periods at a frequency (3.4 ms at 10 kHz, 34 ms at 1 kHz) turns the phase by more than
  half a turn between neighbouring columns, where unwrapping cannot follow it. Find or set
  the delay (**X**, **D**) before reading smoothed phase at high frequencies. A curve set to
  smooth the magnitude only (`ac2 meas new tf … --smooth 6 --smooth-magnitude-only`) says
  `mag only` and keeps the measured phase.
- **Spectra** (narrowband FFT) are smoothed as power over a fractional-octave window on the
  FFT bins. A smoothed spectrum no longer reads as the tone level of a bin — a sine is spread
  over the window and reads lower — so the level axis says so: `dBFS per 1.46 Hz bin (tone,
  1/6 oct smoothed)`. Spectra start unsmoothed (the New spectrum dialog and `ac2 meas new spectrum
  --smooth 6` can set it). At the lowest bins the window is narrower than one bin and the
  bins pass through unchanged.
- **RTA** bands already are fractional-octave: **K** in an RTA says so and changes nothing.
- On a **live measurement** the keys act on the measurement of the focused pane (the
  spectrum pane's when it has focus, else the transfer pane's). The change applies at once
  and the averages carry on — nothing restarts.
- On a **stored trace**: select it — **V** / **Shift+V** step through the shown traces,
  slotted or not, or click it in the **Traces** list (see *Traces and slots*). The row is
  highlighted and the title of the pane it is drawn in names it (`slot 3 (…): smoothing …`,
  `Sweep 2: smoothing off`); then **K** / **Shift+K**. To go back to the live measurement,
  step past the last trace with **V**, click the trace again, choose *Deselect the stored
  trace* in the palette, or select a measurement (**N**, **Alt+1 … Alt+4**, a click).
- From the **command line**: `ac2 trace smooth <trace> 1/12` (also `1/3` … `1/48`, or just
  `12`; `none` turns it off). A transfer or sweep trace keeps the mode it had — magnitude
  and phase for one that was not smoothed — unless `--phase` (magnitude and phase) or
  `--magnitude-only` says otherwise. Exports and `trace show --data` then carry the setting.

Smoothing never changes stored data. A capture keeps the unsmoothed curve and starts with
the smoothing its measurement had, so a trace can be re-smoothed at any time; averages and
A − B combine the unsmoothed curves and start with the smoothing their inputs share.

The banners say what is wrong rather than showing a misleading curve: **NO REFERENCE**, **NO
SIGNAL**, **CHECK ROUTING**, **CLIP**, **STALE** (no fresh frame; the age is shown), **NO
DELAY ESTIMATE**.

## Delay finder

The phase of a transfer function is only readable after the propagation delay from speaker
to mic is compensated. The delay finder estimates it from the impulse response and reports a
**confidence** and the **candidates** it considered:

- **X** finds and inserts the **first arrival** (the direct sound, which is what alignment
  needs); **Shift+X** inserts the **strongest** peak instead.
- When the result is ambiguous (two similar arrivals, a strong reflection), the candidates
  are listed and you pick one with 1–3. When there is no usable estimate, ac2 says so and
  inserts nothing.
- **Arrivals merged into one peak** lists a single candidate: the peak does not have the
  shape of one arrival in that band, because two arrivals lie closer than the band can
  separate or a crossover inside the band smears it (a multi-way box in the full band is the
  usual case). **1** inserts the peak's delay; for the first arrival, run the finder in a
  band without the crossover (palette: *Delay finder: mid / sub band* or a custom band, CLI
  `--band`) and compare.
- **D** types a delay (`12.5ms`, `600samples`, or a distance such as `4.3m`, converted with
  the speed of sound at the set temperature); **,** and **.** nudge the selected trace's
  display by 0.1 ms; **Y** tracks the delay continuously.
- **Ctrl+,** and **Ctrl+.** move the measurement's own delay by one sample, **Alt+,** and
  **Alt+.** by a tenth of a sample. The curve moves at once: the transfer function keeps its
  averages and turns them to the new delay instead of starting over, so you can walk the
  phase into place by eye. Delays are kept to fractions of a sample (the finder's estimate is
  inserted exactly, `600.25samples` can be typed), shown in the measurement list to the
  microsecond.
- A larger change keeps what it can: a stage of the analysis keeps its averages while the
  change is small next to its window (about 2.4 ms at full rate, 9 ms and 28 ms for the
  lower ranges at 48 kHz); only the stages beyond that start over and show *settling*.
- From the CLI: `ac2 delay find main-l --insert`, or limit the band:
  `ac2 delay find sub --band 40hz-120hz`; `ac2 delay nudge main-l -0.25samples` moves it by
  a step.

The inserted delay is also the time origin of the impulse-response pane (**Shift+I** shows or
hides it).

## Traces and slots

A **trace** is a stored snapshot of a measurement's live result, with the metadata needed to
interpret it later: delay, polarity, offset, smoothing, calibration state, mic and time.
Its curve is stored unsmoothed; the smoothing is a display setting you can change later
(see *Smoothing* above).

- **Ctrl+1 … Ctrl+9** capture the selected measurement into slot 1–9 (replacing what was
  there); **1 … 9** show and hide a slot.
- The **Traces** list beside the panes holds every stored trace, slotted or not: its name,
  what it is (*capture*, *sweep*, *imported*, *average*, *A − B*, *target* …), its slot,
  *hidden* when it is, and a dot in its curve's colour (a ring when hidden). Slotted traces
  come first by slot, then the rest oldest first. A click on a row selects the trace (again:
  deselects); a click on its dot shows or hides it.
- **V** / **Shift+V** select the next / previous **shown** trace in that order — sweep
  results and imports included — with the live measurement as the stop between the last and
  the first; **Alt+V** / **Alt+Shift+V** step through the hidden ones too. **Esc** (with no window
  open) goes back to the live measurement — it also stops the stimulus, as always — and
  *Deselect the stored trace* in the palette does the same without touching the stimulus. **A** shows or hides the selected trace. **Move the selected
  trace to slot…** in the palette (`Ctrl+K`) puts it in slot 1–9 (the trace holding that slot
  gives it up; `none` frees its slot), so the digit keys reach it.
- The trace keys act on the selected trace when its curve is on the transfer pane (else on
  the live measurement): **U** inverts it, **,** / **.** nudge it, **E** makes it the phase
  reference, **K** / **Shift+K** smooth it, and **Mic curve on the selected trace…** corrects
  it. The offset keys (**J**, **Alt+↑/↓**, below) act on a selected trace of any kind. A
  target curve takes an offset only (it has no phase); a
  locked trace refuses. The pane's title names the selected trace (`Sweep 2: smoothing
  off`).
- **Spreading curves apart (display offset).** **Alt+↑** / **Alt+↓** move the selected
  curve up / down by 1 dB, **Alt+Shift+↑** / **Alt+Shift+↓** by 3 dB, **Alt+Home** puts it
  back at 0 dB; **J** types a value. The selected curve is the selected stored trace (any
  kind: transfer, target, spectrum / RTA capture, sweep), else the live measurement of the
  focused pane (the spectrum pane's when it has the focus, else the transfer pane's). A
  stored trace's offset is part of its record (shown in `ac2 trace list`, kept in sessions
  and exports); a live measurement's is this app's display only. The toast names the curve
  and its new offset, and the plot says it next to the curve — the transfer legend row
  (`Main L S2 · Δt 0.00 ms · +3.0 dB`), a note in the spectrum pane's corner (`Main L S2 ·
  offset +3.0 dB`), the spectrum cursor values — so a spread is never read as a level
  difference. A locked trace keeps its offset.
- **F2** (or **Rename the selected trace…** in the palette) renames the selected stored
  trace; a double click on a trace in the list selects it and asks for its name in one go.
  `ac2 trace rename <trace> <name>` does the same from the command line.
- **Export the selected trace (ac2 CSV) to a file…** in the palette writes the selected
  stored trace as `ac2 trace export --csv` does: type a file path, or a folder to write it
  under the trace's own name (`Main L S1.csv`). The prompt starts in the folder of the last
  export (at first, the folder the app was started in, where a relative path goes); the
  toast says where the file went. The file is written on this computer, also with a remote
  daemon.
- **Delete** asks before the selected stored trace goes (naming it); **Delete** again or
  **Enter** deletes it, **N** or **Esc** keeps it.
  The selection moves to the next shown trace in the list (else the one before it, else the
  live measurement). A locked trace is not deleted, and Delete never deletes a live
  measurement (that is **Delete selected measurement** in the palette). The palette has
  **Delete selected trace…** too.
- **One selection for the sweeps:** a sweep selected in the list or with V is the one the
  **Sweep / distortion** pane shows, and **N** / **Shift+N** on that pane select the sweep they
  step to, for the transfer pane and the trace keys too. A finished sweep is selected.
- Overlays are drawn relative to the selected trace's measured delay, so relative arrival
  times between traces stay visible.
- **C** turns on the comparison cursor, synchronised across panes and traces; **Shift+←/→**
  moves it.
- **M** averages the shown stored traces (power; complex and coherence-weighted averages are
  in the command palette); A − B is a dB difference, A / B a complex division.
- **Z** loads a target curve; the command palette imports CSV and other analyzers' text
  exports. `ac2 trace export <name> --csv out.csv` exports.
- From the command line: `ac2 trace display <trace> on|off` shows or hides a trace, `ac2
  trace slot <trace> 3` puts it in slot 3 (`none` frees its slot); `ac2 trace list` shows
  both.

### Export and import

`ac2 trace export <trace> --csv out.csv` (`-` for stdout) writes the ac2 CSV: a `#` header
with every metadata field, then one row per column. The columns are always **as measured**:
offset, polarity, nudge, smoothing and a mic curve put on afterwards are listed in the header
but not applied, so an export re-imports exactly. `ac2 trace import out.csv` brings it back
with its name, kind and **delay** (the delay the phase is referred to, `# delay_ms:`); the
other display settings start fresh. A **sweep** export also holds the sweep's analysis facts
and its impulse response, and imports as a sweep again — the Sweep / distortion pane and the
IR view draw it as they drew the original. A sweep exported by an older ac2 (no
`# sweep_info:` line) imports as its transfer function with the delay; the distortion is
dropped and the import says why (`note:` in the CLI output, and in `ac2 trace show`).

### Mic curve on a stored trace

A trace captured before the mic had a curve — or with no mic name, or with the input's
mic curve off — can be corrected afterwards: `ac2 trace mic <trace> "MM1 34804" --label 90°`
applies that curve of the mic library (the label may be left out when the mic has one
curve), and `ac2 trace mic <trace> none` takes it off again. In the app: select the trace
(Traces list or V), then **Mic curve on the selected trace…** in the palette (`Ctrl+K`), prefilled with the
trace's mic; type the curve's label after it (*MM1 34804 90°*). Like smoothing it is a
display setting: the stored curve stays as measured, the correction (0 dB at the calibrator
frequency, else 1 kHz) is applied when the trace is shown, and `ac2 trace show` reads `mic
MM1 34804 (curve 90° applied after capture, 0 dB at 1000 Hz, file …)`. The curve's points are kept with the trace, so deleting or
replacing the curve in the store later does not change the trace. A sweep's distortion is
corrected too (each harmonic is picked up at its own frequency). A trace captured **with**
the curve already applied (`mic … (curve … in the columns)`) refuses a second one — it would
correct twice. Averages and A − B combine the corrected curves.

## Sweep measurement: response and harmonic distortion

A sweep measures a speaker's response and its **harmonic distortion** (H2 … H5 and THD vs
frequency) in a few seconds. The generator plays a synchronised exponential sine sweep on the
speaker's output and on the loopback output; ac2 records the loopback (reference) and the mic,
divides one by the other, and separates the harmonics, which arrive before the linear impulse
response. Design and accuracy: [sweep-distortion.md](design/sweep-distortion.md).

- **App:** **Shift+S** (or **Sweep measurement** in the palette) opens the dialog: reference,
  mic and the speaker's output by name (the session's loopback output always plays too), the
  **level** (typed, no default), 20 Hz – 20 kHz, duration (1 s quick look, 3 s default; 6 s
  and 12 s lower the noise floor), repeats (each doubling lowers the floor by 3 dB), name.
  The reference is the session's loopback input; a session without a loopback mapping leaves
  it as "choose the reference", and the sweep does not arm until one is picked. **←/→** step
  a choice (→ longer / more) and stop at the ends; a text field's text is selected when it
  gets the focus (**Ctrl+A** selects it again), so typing replaces it. **Enter** arms the
  sweep (the dialog closes), **Enter** again plays it, **Esc** stops (and discards it).
  **Esc** in the dialog closes it; anything armed and not yet playing is disarmed with it,
  so nothing stays armed behind a closed window, while a stimulus already playing keeps
  playing (**Shift+Esc** or the strip's **Stop** stops it). Once the sweep has played
  the stimulus is off (STIM OFF) and the lease is given back: nothing stays armed, and
  **Shift+S** sets up the next sweep. The result opens the
  **Sweep / distortion** pane (**Alt+5**): the fundamental's response above, the distortion
  below. Where an order is within the noise it is drawn dashed at its own floor; the shading
  is the noise under every order's floor. The **dB | %** toggle in the pane's title (or
  **U**) switches the distortion between dB re fundamental and percent (a log axis: 0.01 %,
  0.1 %, 1 %, …), readouts included. **Shift+I** shows the
  sweep's impulse response with the harmonics' impulses marked (**G**: linear / log / ETC),
  **N** steps through stored sweeps (selecting each), **Shift+W** hides the pane. The sweep is also a stored
  trace, drawn in the transfer pane like any capture and listed under Traces (see *Traces and slots*).
- **Progress strip:** while a sweep runs (from this app, another client or the CLI), a strip
  under the top bar — visible whichever pane is maximised — shows its name and level,
  *sweep 1 of 2*, a bar and the time left (about the remaining repeats × (sweep + the
  silence after it), counted from when each repeat began), then *analysing…*. Its
  **Stop (Shift+Esc)** button, like **Shift+Esc** (from anywhere) or **Esc** (with no window
  open), fades the output out, disarms the generator and discards the run; nothing is
  stored.
- **CLI:** `ac2 ir capture --ref 2 --mic 1 --out 1,2 --level -50dbfs` (`--from 20hz --to
  20khz --duration 3s --repeats 1 --gate 5ms --name …`). Like `gen`, it arms and waits:
  **Enter** plays, **Esc**/**q**/**Ctrl-C** stops. The daemon disarms the generator as soon
  as the sweep has played (or failed); any client arms again for the next one. It then prints THD at 100 Hz, 1 kHz and
  10 kHz and each order's highest point; `--json` gives the same as JSON lines.
  `ac2 trace export <sweep> --csv out.csv` writes every curve (response, each order and its
  floor, THD), the analysis facts and the impulse response; `ac2 trace import` of that file
  restores the sweep. The sweep's columns are uncorrected even when its mic has a curve:
  `ac2 trace mic <sweep> <mic>` applies it.
- **Room parameters (ISO 3382-1):** every sweep also computes EDT, T20, T30, C50, C80 and
  D50 of its impulse response per octave band (and one-third octave) and broadband
  (`docs/design/room-metrics.md`). In the app, the sweep pane's impulse response
  (**Shift+I**) shows the octave table under the plot when the pane is tall enough. A value
  the measurement cannot support is a word, never a number: `noise` (the decay meets the
  noise too soon: T30 needs 45 dB of decay range, T20 35 dB, EDT and C/D 20 dB), `short`
  (the decay is too short for that band's filter), `—` (no decay, e.g. an anechoic
  measurement); T30 with `*` is a curved decay (more than 10 % above T20). The silence after
  the sweep must hold the room's decay: the dialog's **Silence after** (1, 2, 4, 8 s) or
  `--tail 4s` on the CLI; a hall of 2 s wants 4 s. `ac2 ir metrics <sweep> [--third]
  [--json]` prints the table; `ac2 ir capture` prints the octave table after its summary;
  the CSV export carries them.
- A distortion value is only shown where it is at least 6 dB above the noise in its window;
  elsewhere it reads `< −72.0 dB` (`< 0.0251 %`: the floor). Lower the floor with repeats or
  a longer sweep, not with more level than the speaker should take.

## Sessions

`ac2 session save <name>` (or **Session: save** in the palette) stores the measurements and
traces, including slots, display edits (smoothing, a mic curve applied after capture; curves
are saved as measured), sweep distortion and impulse responses, and each SPL meter's
per-second log, in the daemon's session directory (`sessions/` in the ac2 data directory);
`ac2 session load <name>` (**Session: load** in the palette) restores them and
`ac2 session list` lists them. A path instead of a name saves or loads anywhere on the
daemon's machine. Calibrations are not part of a session: they describe the machine's
hardware and stay in its calibration store. A session saved with a different session format
is refused with the version named. A loaded session always comes up disarmed: nothing plays
until someone types a level and fires.

### Autosave

A stand-alone daemon (`ac2d`, `ac2 daemon start`, the `ac2d` user service) also **autosaves**
the same content — measurements, traces with sweep distortion and impulse responses, slots,
display edits — shortly after every change (1.5 s after the last edit of a burst, at most
10 s after the first) and once more when it stops. When it starts again, for example after
`--max-level` was changed, it loads that autosave exactly like `session load`: disarmed, no
audio session opened (open one as usual; restored measurements wait for it). Its log says
what was restored.

The top bar shows the state next to the stimulus: *autosaved just now* / *autosaved 5 min
ago*, *saving…* while a change is being written, and *autosave failed: <reason>* in warning
colour when the disk refused (it retries; hover for the whole reason). `ac2 status` and
`ac2 session status` print the same line. The daemon hosted inside the app does not autosave;
save a session by name there.

Files, in the ac2 data directory (`~/.local/share/ac2` on Linux, `~/Library/Application
Support/ac2` on macOS, `%APPDATA%\ac2\data` on Windows):

| | |
|---|---|
| `autosave/` | the current autosave (a session directory); inside it `session.prev.json` is the one before, used should the newest be damaged |
| `autosave.v<N>/` | an autosave of another session format, set aside at start, not deleted |
| `autosave.damaged/` | an unreadable autosave, set aside |
| `autosave.unrestored/` | what `ac2d --no-restore` did not load |

Any of these loads by path: `ac2 session load ~/.local/share/ac2/autosave.unrestored`.

The autosave is kind to SD cards and batteries: a write adds only the traces that changed,
and an SPL meter's per-second log is a file the daemon appends to — the new rows every 30 s,
synced to the card every 5 minutes and when the daemon stops. A running meter does not make
the top bar say *saving…*. After a power cut a log is back up to its last few minutes; after
a crash of the daemon alone, up to its last 30 s.
`ac2d --no-restore` starts empty, `--autosave <dir>` puts the autosave elsewhere (one daemon
per directory), `--no-autosave` keeps everything in memory only.

## Calibration and SPL

Inputs carry a **mic name** (**N** on the input in the session dialog or in the
Calibrations view, `ac2 session open … --mic 3=M30`, `ac2 session inputs --mic 3=M30`).
Two things are stored, in the calibration store of the daemon's machine:

- **Sensitivity** (dB SPL of 0 dBFS) — per device, input and mic. It calibrates the whole
  chain, preamp gain included, so it belongs to that input and that gain: change the gain
  and calibrate again. There are two ways to get it:
  - **acoustic**: put a 94 dB (or 114 dB) calibrator on the mic and run
    `ac2 cal spl --input 3 --ref 94db` (`--freq` when the calibrator is not 1 kHz). This is
    the reference method; the app has no calibrator dialog yet, so it is done from the CLI;
  - **electrical** ([below](#calibrating-without-a-calibrator-electrical)): no calibrator,
    but a true-RMS voltmeter at the input and the mic's data-sheet sensitivity, with a stated
    uncertainty (±1 dB). **E** in the Calibrations view, or `ac2 cal electrical`.

  SPL is then computed from the raw input level with that sensitivity. A calibration made
  with another mic or on another input is used but shown as such; an acoustic calibration
  replaces an electrical one, never the other way round unless you say so.
- **Mic curves** (the **mic library**) — per mic, any number, each with a short **label**: a measurement mic often
  comes with one file per incidence angle (0° for pointing at the source, 90° for grazing
  incidence), and using the wrong one is a few dB of error at high frequencies.
  `ac2 cal curve import 449350_34804_90Grad.txt --input 3` imports a file into the mic
  library as a curve of input 3's mic (`--mic NAME` instead names the mic directly). The
  label comes from the file — the angle its header or name states (*90-degree-curve*,
  `_90Grad`, `0deg` → *90°*, *0°*), else the file name — or from `--label`;
  `ac2 cal curve rename --mic "MM1 34804" 0° "on axis"` renames it later. A sensitivity the
  file states (*15.0 mV/Pa = −36.5 dBV*) is shown as the data sheet value; it is used only
  as the default sensitivity of an electrical calibration. Curves follow the mic name to any input and device.

**Which curve is in use** is chosen per input, explicitly: `ac2 cal use 3 90°` (or `off`;
`ac2 session inputs --mic 3=M30 --curve 3=90°` sets names and curves of several inputs at
once, and `ac2 session inputs` alone lists them); in
the app **←/→** on the input's row in the session dialog or in the **Calibrations** view
(palette; *Input setup…* opens it on the selected measurement's input), **Mic curve on input N…** (*3=90°*), or **Mic curve: next curve on the selected
measurement's input**. Importing a mic's first curve on an input chooses it; with several
curves and none chosen, none applies and the input says *choose: 0°, 90°* — ac2 never
guesses. The change applies to the running measurements at once (it is a display correction
of the magnitude, so averages need no reset). Wherever a corrected readout is shown it says
which curve is in it — the input's label in the sidebar (*MM1 34804 · 90° · mic (in 1)*), the
transfer, spectrum, RTA and SPL captions (*mic curve: MM1 34804 90°*), `ac2 cal list` and
`ac2 status` — or why there is none (*mic curve off*, *no mic curve stored for MM1 34804*,
*mic curve 90° not stored for MM1 34804* after the curve was deleted). Captured traces keep
exactly which curve their columns carry (label, file and content hash; the export header
says it).

### The Calibrations view

The **Calibrations** view (palette; **Input setup…** opens it on the selected measurement's
input) lists what each input uses, every mic with its curves (file, points, range, data
sheet sensitivity, which inputs use it) and every sensitivity calibration (device, input,
mic, calibrator level and frequency, reading, age). **↑/↓** move; **←/→** choose an input's
curve; **N** names the mic on an input; **I** imports a curve file for the focused mic (type
the path); **R** renames a curve; **E** calibrates an input electrically (below); **Delete** (twice) deletes a curve or a sensitivity
calibration. On the command line: `ac2 cal list`, `ac2 cal curve rm --mic NAME LABEL`,
`ac2 cal rm --input 3` (a sensitivity calibration).

A calibration store written by an older ac2 is set aside (renamed to
`calibrations.json.v1`, `.v2`, …, never deleted) and the daemon starts with an empty store: calibrate
again and import the curves again; the old file shows the mic and file names.

### What the calibration labels say

Every calibrated readout names what its dB SPL rests on: the SPL meter's footer, the Leq
caption, the spectrum / RTA captions, the Calibrations view, `ac2 cal list` and
`ac2 status`.

| label | meaning |
|---|---|
| `cal 94 dB · 3 h ago` | acoustic: a 94 dB calibrator on this mic on this input, 3 h ago |
| `verified · 94.0 dB SPL at 1.00 kHz · 3 h ago` | the same, written out (Calibrations view, session dialog) |
| `electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB · 2 h ago` | electrical, a voltmeter across the connected, powered mic's pins 2–3; the sensitivity taken from the mic's curve file (`data sheet`) or typed (`typed`); ±1 dB the stated uncertainty |
| `electrical cal (injected, 10.0 mV/Pa) ±0.5 dB` | electrical, a generator in place of the mic, phantom off |
| `cal from other mic / input`, `from M30 on in 2 · …` | a calibration made with another mic or on another input: used, but it may not hold here |
| `uncalibrated` (values in dBFS) | no calibration for this device, input and mic |
| `· mic curve` after any of these | a mic curve is in the readout as well (its label is in the caption) |

Limits of Leq windows are judged only on a calibrated input, whichever method.

### Calibrating without a calibrator (electrical)

No 94 dB calibrator at hand, but a true-RMS multimeter (or an Analog Discovery 2)? ac2
reads the input's level while you measure the voltage at the same input; the mic's
sensitivity (mV/Pa, from its data sheet) turns that into dB SPL:

    V_FS = V / 10^(L / 20)                 volts at 0 dBFS (V measured, L read in dBFS)
    dB SPL = dBFS + 20·lg(V_FS / (S · 20 µPa))     S = mic sensitivity in V/Pa

Example: 15.0 mV read at −40.0 dBFS → 0 dBFS = 1.500 V; with 15.0 mV/Pa that is 100 Pa, so
0 dBFS = 134.0 dB SPL, and the mic at 1 Pa (94 dB) gives the 15 mV that reads 94.0.

It counts as a calibration everywhere — dB SPL, Leq limits judged — and says what it rests
on wherever it is shown: *electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB · 2 h ago*
(an acoustic one says *cal 94 dB · 2 h ago*). The ±1 dB is mostly the data sheet's: a
capsule's sensitivity tolerance is typically ±0.5…1 dB (an individual calibration sheet of
your mic is better: then state `--uncertainty 0.5db`); the meter adds a little (see its
accuracy at the range and 1 kHz). An acoustic calibration later replaces it; it never
replaces an acoustic one unless you say so (`--replace-acoustic`, or Enter twice in the app).

**In-line (the usual way: the mic stays connected and powered).**

1. Put an XLR breakout (in-line adapter with test points) between the mic cable and the
   input. Phantom power stays **on**.
2. Set the preamp gain you will measure with. Do not change it afterwards (a gain change
   needs a new calibration).
3. Play a steady **1 kHz** sine through a speaker at the mic, loud enough that the meter
   reads well above its range floor (a handheld DMM: ≥ 5–10 mV) and the input stays below
   −3 dBFS. Your own tone source, or ac2's generator (arm and fire as usual; its level
   ceiling applies). The mic and the speaker must not move during the reading.
4. Set the meter to **AC volts** (true RMS; AC-coupled, which also blocks the phantom
   voltage), its lowest range that fits; check its stated accuracy at that range and 1 kHz.
   Measure **between pins 2 and 3 only**. Phantom power puts +48 V on both pins 2 and 3
   against pin 1: never measure to pin 1, never short pins together (that is what the
   breakout is for).
5. Calibrate while the tone plays: in the app, **Calibrations…** (palette), the mic's input,
   **E**; type the voltage the meter shows (*15.03 mV*), check the sensitivity (prefilled
   from the mic's curve file when it states one, else type the data sheet's, e.g. *15.0 mV/Pa*
   or *−36.5 dBV/Pa*), **Enter**. On the command line:
   `ac2 cal electrical --input 2 --volts 15.03mv` (the sensitivity from the data sheet in the
   mic's curve file, or `--sensitivity 15.0mv/pa`). ac2 refuses while the level is not yet
   steady (retry after a few seconds), without signal, when the input clipped, above
   −3 dBFS or below −70 dBFS.

In-line includes the mic's real source impedance loading the preamp, which an injected
signal does not.

**Injected (a generator in place of the mic).** An Analog Discovery 2's waveform generator
(or any sine source) drives the input directly:

1. **Switch phantom power OFF on that input first.** 48 V on the XLR can damage the
   generator; ac2 cannot switch phantom power.
2. Wire the generator between pins 2 (hot) and 3 (cold, also to pin 1 for an unbalanced
   generator), the gain as you will measure with.
3. Play 1 kHz at a level near what the mic gives (tens of mV), measure the voltage at the
   XLR pins 2–3 with the scope channel or a DMM, and run
   `ac2 cal electrical --method injected --input 2 --volts 15.0mv` (in the app: ←/→ on
   *Measured* chooses injected).
4. Disconnect the generator, plug the mic back in and **switch phantom power back on**.

Small voltages are hard to read accurately on a handheld meter. **Divider tip:** set the
generator to about 1 V, measure that precisely, and feed the input through a precise
divider (e.g. 0.1 % resistors, 10 kΩ : 100 Ω ≈ 1 : 101) — the input voltage is the measured
one times the divider ratio. Measure the divider's output loaded by the input when you can.

A tone other than 1 kHz is accepted (`--freq 400hz`; a meter specified only to 400 Hz):
the sensitivity is still the capsule's at 1 kHz, so this assumes the preamp is flat between
the two — ac2 notes it. The mic curve stays normalised at 1 kHz.

### SPL meter

The **SPL meter** shows the sound level with Fast, Slow or Impulse time weighting and A, C
or Z frequency weighting (IEC 61672-1: F and S are exponential averages of the squared
signal with 125 ms and 1 s; I, from IEC 60651, averages with 35 ms and holds peaks, falling
with 1.5 s), with Leq, LCpeak, Lmax and Lmin, in the SPL pane or in the terminal. A new
meter reads **A-weighted, Fast** (`LAF`) unless you choose otherwise, in the app and with
`ac2 meas new spl` alike.

- **Three views, G steps them**: the **meter** (below), the **Leq windows** (next section)
  and **meter + Leq**, where a new meter starts: the meter's number centred across the top
  third of the pane — its name and unit (`LAF · dB SPL`) under it and the live bar, the same
  number with the same hold as in the meter view, without its statistics — and the Leq
  windows below it as columns or tiles (**B**, **Shift+B** as in the Leq view), under one
  caption for both: the meter's name, the run and the calibration. G goes meter → Leq
  windows → meter + Leq → meter; the palette has each by name ("SPL pane: meter + Leq
  windows"), and the app remembers the choice. W twice (or W, then F11) makes it the stage
  view: the number and the windows, nothing else (the top bar comes back while a stimulus is
  armed or playing). On a short pane the number and its name share one line above the
  windows; on a very short one (under about 170 px) the number gives way: the windows judge the
  limits, the meter view still has it.
- **The number** is the current time-weighted level, centred and as large as the pane
  allows; under it the level's name and unit, `LAF · dB SPL` (`dBFS` uncalibrated), then a
  slim bar with the level live (30 … 130 dB SPL, or −100 … 0 dBFS, 10 dB ticks), then the
  meter's own statistics under one heading that says since when they run — *meter since
  4:01 · R resets* (local time; the date too when not today) — `LAeq`, `LCpeak`, `LAFmax`
  and `LAFmin` in the meter's weightings, and at the bottom the calibration. These are the
  meter's figures since its start or the last **R**, not the Leq windows (**G**), which
  keep their own lengths and are never reset by R.
  The secondary figures grow with the pane: **W** twice (or W, then F11) makes the meter
  full screen, to be read across the room.
- **Readable, not flickering.** The number takes a new reading twice a second with F and I
  and once a second with S, as a hand-held meter's display does; the bar moves with every
  frame. The reading is the time-weighted level at that instant — the time weighting is
  the averaging, displayed values are never averaged in dB. (I needs no longer hold: its
  1.5 s fall holds peaks itself.) `spl_hold_ms = 250` in `ui.toml` sets another display
  period (100 … 10000 ms) for every time weighting.
- **F** in the SPL pane steps the time weighting Fast → Slow → Impulse, **Z** the frequency
  weighting A → C → Z; the palette has each one by name ("SPL meter: Slow time
  weighting", "SPL meter: C weighting"…). The change applies to the running meter at once
  and is kept with it (sessions, autosave). The pane stays as it is: with the Leq windows
  showing, a message names the meter's new reading (*FOH SPL: LCS*) and the windows stay —
  they keep their own weightings. Nothing restarts: the meter measures every
  combination all the time, so the new one reads its settled level from the first frame
  (a Slow meter started at the switch would need 5 s), and its Lmax, Lmin, Leq and Lpeak
  cover the same interval as before — each combination keeps its own, from the meter's
  start or the last **R**. The Leq windows and the per-second log carry on untouched.
- `ac2 spl set --weight c --time slow` does the same from the terminal (`--meas` or
  `--input` when there is more than one meter). `ac2 spl watch --input 3 --weight a` shows
  a meter in the terminal (add `--json` for one JSON line per update, `--for 10s` to stop on
  its own). With `--input` the command runs its own meter for as long as it runs, so Leq,
  Lmax and Lmin cover exactly what it watched; `--meas` shows an existing meter instead.

### Leq windows and limits

Every SPL meter also keeps **rolling Leq windows** — by default LAeq over 1, 5, 10, 30 and
60 min — and a **per-second log** (LAeq, LCeq and LZeq of every second, the last 48 hours).
Both run as long as the meter runs, whether or not any app or terminal is watching, and carry
on when the meter is stopped and started, its windows are changed, the session is reopened or
the daemon restarts (the log is in the autosave and in saved sessions).

- **G** in the SPL pane steps meter → windows → meter + windows (above). The windows show as
  **columns**, made to be read from the stage or across the room: one full-height column per
  window, the shortest on the left, each a bar that fills from the bottom with the window's
  Leq, the value in large digits on top with the window's own unit and weighting beside it
  on the same line, small and dim — **dB(A)**, **dB(C)**, **dB(Z)**, or **dBFS (A)**
  uncalibrated — and the window's
  name at the bottom ("LAeq 30 min", shortened to "30 min" or "30m" when the columns are
  narrow — the caption then says "LAeq"). The unit is never left out: in meter + Leq the
  meter above may read in another weighting (LCS over LAeq windows), and a column must not be
  read in the meter's. Tiles show it the same way. The limit is a line across the column; a column turns **amber** within the warn
  margin (3 dB by default) of its limit and the whole column goes **red** above it, and goes
  back when the window recovers; each going over and each recovery also shows as a message.
- A window **still filling** (a new log, a longer window than the meter has run) shows its
  Leq **so far** ("so far · 12:30 / 30:00") but is judged on its **budget**: the limit allows
  so much sound energy over the whole window, and the column goes **red** only once that is
  spent — when the window will end over its limit even if everything is silent from now
  on. Before that, a Leq so far above the limit turns it **amber, "ON COURSE — over in
  12 min"**: at the same level the budget runs out in 12 minutes, and turning down now
  still keeps it under. Its bar shows the budget spent (the level the window would end at
  if the rest were silent), climbing to the limit line as the budget runs out, and its
  headroom is the level that would use up exactly what is left by the time the window is
  full ("stay ≤ 98.2 dB"; the CLI says "until full: …"). For example, 70 dB against 60 dB limits on a fresh log is
  ten times the limit's power: the 1 min window is red after about 6 s, the 60 min one
  after about 6 min, amber on course before then. Seconds not measured neither spend nor
  add to the budget. The regulations define their limits on full windows only; red only
  when going over is certain is ac2's choice for the time before. All columns share one
  scale so their bars compare: from 30 dB below the
  (lowest) limit to 6 dB above the (highest); without limits, or uncalibrated, a 40 dB range
  that follows the loudest window in 10 dB steps and stays put from second to second.
- **B** switches between columns and **tiles** (a grid with every figure written out),
  **Shift+B** shows or hides the **history strip** below them: each window over time against its limit
  (dashed), red where it was over. It holds the log's last 4 hours even when the app was
  not running: a restarted (or reconnected, or second) app gets it from the daemon, rebuilt
  from the meter's log as the meter computed it; a new log clears it. The app remembers
  both. **W** gives the pane the whole
  window, once more (or **F11**) the whole screen: the **stage view**, nothing but the
  columns (in meter + Leq, the number above them) and the caption with the meter's name,
  the run and its calibration (the top bar comes
  back while a stimulus is armed or playing). W again goes back to the split layout.
- Each column (and tile) says, large, its **state** (OK, NEAR, OVER, or "over in 47 s" when a
  filling window is on course to go over) and the **headroom**: the highest steady level for
  the next minute that keeps the window at or below its limit (**"stay ≤ 101.5 dB"**; while
  the window fills for longer than that, the level that keeps it under until it is full),
  or, over and unable to recover within the minute, **"cooling down in 7 min 30 s"**: the
  time until the window is back under its limit if the level stays at the limit. Its limit
  is written small under them, and is the line across the bar. The window's **Leq** is a
  small figure low on its bar that stays put while the bar moves — in white or black,
  whichever reads on what is behind it — so the meter's own number above the windows stays
  the one big number; what to do about a window is the headroom. While the window fills, how much of it there is;
  and **"offline for 1 min 20 s"** when part of it has no audio at all (the meter stopped,
  the daemon was down, the capture lost samples). Missing audio isn't counted as silence:
  the window's Leq is the average of what was measured, and offline time neither lowers it
  nor spends or earns budget. The note goes away once that time has slid out of the window.
  Narrow columns use the shorter wordings, or leave a line out.
- The caption above the windows, centred, says how long the meter has been logging and the level of
  the whole log: **`running 2:14:05 since 19:02 · LAeq total 97.8 · offline 12 s`** — the
  time since the log's first second (it keeps counting when the app or the daemon is
  restarted: the log comes back with the autosave), its start in local time, the energy
  average over everything measured (LCeq and LZeq too when a window uses them; dB SPL when
  calibrated), and the time not measured, if any (the meter stopped, the daemon down, lost
  samples — never counted as silence). It is large in the stage view and shortened in
  narrow panes (`2:14:05 · total 97.8`). After 48 hours the log keeps its last 48 hours and
  the caption says "last 48 h".
- **Shift+R** in the SPL pane (or "Start a new SPL log…" in Ctrl+K) starts a **new log** — for
  the show after a loud soundcheck: the windows, their states, the alarms, the clock and the
  total start over; the windows and limits stay. It asks first, naming the run that ends.
  The ended log can still be exported (`ac2 spl leq export --previous`) until the next new
  log or a daemon restart.
- **Shift+L** (or "Leq windows and limits…" in Ctrl+K) sets them: the window lengths and
  weightings picked with ←/→, limits and warn margins typed in dB (empty: no limit), a
  **preset** row and the headroom horizon. ↑/↓ moves between rows, Tab between cells,
  **Insert** adds a window, **Delete** removes one, Enter applies.
- A preset **replaces the windows** with exactly the rule's — its windows and limits, and a
  window it wants shown without a limit — shortest first; windows and limits the rule does
  not state go. Informational only — not legal advice: each rule also has peak limits, a
  measuring position and duties of its own (`docs/design/leq.md` lists them). ←/→ on the
  preset row shows each preset's windows; back at "none" the windows return as they were;
  editing a window keeps the preset's. **Insert** adds more windows afterwards. The log
  carries on: the new windows are rebuilt from it.

  | preset (`--preset`) | windows and limits |
  |---|---|
  | DIN 15905-5 (`din15905`) | LAeq 30 min ≤ 99 dB |
  | Swiss V-NISSG (`swiss93`, `swiss96`, `swiss100`) | LAeq 60 min ≤ 93 / 96 / 100 dB |
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

  Wallonia has no preset: its 2018 rule is not in force. No measuring-position correction
  is applied: a mic at FOH reading for the loudest audience position (or for the rule's
  measuring position) needs the difference added to the limit by hand.
- Limits are judged only on a calibrated input (dB SPL, see above); an uncalibrated meter
  shows its windows in dBFS, marked "not calibrated".

In the terminal: `ac2 spl leq watch` (big numbers; `--json` for one line per second),
`ac2 spl leq set --preset france --windows 1min --limit 1min=102db` (the preset's windows
and an LAeq 1 min of your own; several `--preset` give the windows of all, a shared window
at the lower limit; without `--preset`, `--windows 1min,5min,c:30s` sets the windows; also
`--warn 3db`, `--horizon 1min`), `ac2 spl leq export -o show.csv` (the per-second log as
CSV, for the record), `ac2 spl leq new --yes --export soundcheck.csv` (a new log, the ended
one written first; without `--yes` it only says what would end). Each takes `--meas` or
`--input` when there is more than one meter.
How it is computed: `docs/design/leq.md`.

## Keyboard

Everything in the app is reachable from the keyboard. **H** (or **F1**) shows the bindings,
**Ctrl+K** opens the command palette, which finds every command by name and shows its key.
Keys are scoped: the focused pane's keys apply first, then the global ones. The defaults
avoid `[ ] + - =` and other keys that need AltGr or a dead key on Nordic and other European
layouts. The stimulus cluster is fixed: **Space** arm, **Enter** fire, **Esc** stop, **↑/↓**
level (±1 dB, with Shift ±3 dB). With **Alt** the arrows move the selected curve's display
offset instead, with **Ctrl** they pan the focused pane's level axis.

**An open window owns the keyboard.** With the help, the palette, a prompt, a dialog or the
calibrations view open, **↑/↓** move the selection or scroll (**PageUp / PageDown**,
**Home / End** where a list is long: the help, the palette, the calibrations view, a pane's
measurement list), **←/→** change a choice, **Enter** confirms and **Esc** closes the
topmost window only — the electrical calibration dialog closes back to the calibrations
view. None of these keys reaches the stimulus: ↑/↓ never change the level and Esc never
stops while a window is open, and the mouse wheel scrolls the window rather than zooming the
plot behind it. The help and the delay candidates leave the other keys working (try a key
while reading), except the stimulus's. **Shift+Esc** stops the stimulus from anywhere,
windows included; it is fixed, and cannot be rebound.

In the delay candidate list ↑/↓ and Enter choose a candidate; 1–3 pick one directly.

Change bindings in `keys.toml` in the ac2 config directory (`~/.config/ac2` on Linux,
`~/Library/Application Support/ac2` on macOS, `%APPDATA%\ac2\config` on Windows):

```toml
[global]
cycle_theme = "Ctrl+T"

[transfer]
insert_delay = ["X", "Alt+D"]
```

A `keys.toml` that cannot be used — a conflict, an unknown key or command, a pane's command
on **H** (help in every pane), a change to the fixed **Shift+Esc** — is reported at start
and the default keys are used until it is fixed.

**Key hints.** The focused pane shows a slim line under its plot with its most used keys,
for example in the transfer pane `V select trace · A show/hide · Ctrl+1 capture · X find
delay · K smoothing · Shift+I IR · W maximise · Alt+↑ offset · H all keys`. The keys are the ones bound now
(a key changed in `keys.toml` shows its new chord; macOS shows `⌘ ⌥ ⇧`). On a narrow pane the
least used drop off first; **H all keys** always stays. Hovering over a pane's name (or the
line) lists the same keys with what each does; hovering over a clickable control (the
**dB | %** toggle, **Stop**, a trace's colour dot or row, a measurement, a pane's measurement
chip) names the key that does the same. **Shift+H** (palette: *Key hints on / off*) turns the
line off and on; the app remembers it in `ui.toml` (`key_hints = false`). The stage view
never shows it. Every pane's line is listed at the end of the keyboard map below.

### Keyboard map

<!-- keymap:begin (generated by crates/ac2-ui/tests/keymap_doc.rs) -->
Keys as on Linux and Windows; on macOS `Ctrl` is `⌘` and `Alt` is `⌥`. Every binding can be changed in `keys.toml` using the names in the last column.

#### Everywhere

| Keys | Command | `keys.toml` |
|---|---|---|
| `H` or `F1` | Show / hide key bindings | `help` |
| `Ctrl+K` | Command palette | `palette` |
| `Ctrl+Q` | Quit | `quit` |
| `F11` | Window full screen on / off | `fullscreen` |
| `Shift+H` | Key hints on / off | `key_hints` |
| `Space` | Stimulus: arm (needs a typed level) | `stimulus_arm` |
| `Enter` | Stimulus: fire (when armed) | `stimulus_fire` |
| `Esc` | Stimulus: stop and disarm (no window open) | `stimulus_stop` |
| `Shift+Esc` | Stimulus: stop and disarm, also with a window open | `stimulus_stop_anywhere` |
| `↑` | Stimulus level +1 dB | `level_up` |
| `↓` | Stimulus level −1 dB | `level_down` |
| `Shift+↑` | Stimulus level +3 dB | `level_up_coarse` |
| `Shift+↓` | Stimulus level −3 dB | `level_down_coarse` |
| `L` | Stimulus: type level (dBFS)… | `stimulus_level` |
| `Alt+1` | Focus transfer-function pane | `focus_transfer` |
| `Alt+2` | Focus spectrum / RTA pane | `focus_spectrum` |
| `Alt+3` | Focus impulse-response pane | `focus_ir` |
| `Alt+4` | Focus SPL pane | `focus_spl` |
| `Alt+5` | Focus (and show) the sweep / distortion pane | `focus_distortion` |
| `Tab` | Focus next pane | `next_pane` |
| `Shift+Tab` | Focus previous pane | `prev_pane` |
| `W` | Layout: split → one pane → full screen | `maximize_pane` |
| `N` | Select next measurement of the focused pane | `next_measurement` |
| `Shift+N` | Select previous measurement of the focused pane | `prev_measurement` |
| `T` | Theme: dark → light → high contrast | `cycle_theme` |
| `I` | Zoom frequency in | `zoom_in` |
| `O` | Zoom frequency out | `zoom_out` |
| `←` | Pan frequency down | `pan_left` |
| `→` | Pan frequency up | `pan_right` |
| `Home` | Reset zoom (20 Hz – 20 kHz) | `reset_view` |
| `Ctrl+I` | Zoom level axis in (vertical) | `level_zoom_in` |
| `Ctrl+O` | Zoom level axis out (vertical) | `level_zoom_out` |
| `Ctrl+↑` | Pan level axis up (towards higher levels) | `level_pan_up` |
| `Ctrl+↓` | Pan level axis down (towards lower levels) | `level_pan_down` |
| `Shift+Home` | Fit level axis to the shown curves, frequency to 20 Hz – 20 kHz | `level_fit` |
| `Ctrl+Home` | Level and frequency axes back to the pane's default | `level_reset` |
| `C` | Comparison cursor on / off | `toggle_cursor` |
| `Shift+←` | Cursor 1/12 octave down | `cursor_left` |
| `Shift+→` | Cursor 1/12 octave up | `cursor_right` |
| `Ctrl+1` | Capture selected measurement to slot 1 | `slot_1` |
| `Ctrl+2` | Capture selected measurement to slot 2 | `slot_2` |
| `Ctrl+3` | Capture selected measurement to slot 3 | `slot_3` |
| `Ctrl+4` | Capture selected measurement to slot 4 | `slot_4` |
| `Ctrl+5` | Capture selected measurement to slot 5 | `slot_5` |
| `Ctrl+6` | Capture selected measurement to slot 6 | `slot_6` |
| `Ctrl+7` | Capture selected measurement to slot 7 | `slot_7` |
| `Ctrl+8` | Capture selected measurement to slot 8 | `slot_8` |
| `Ctrl+9` | Capture selected measurement to slot 9 | `slot_9` |
| `1` | Show / hide slot 1 | `show_slot_1` |
| `2` | Show / hide slot 2 | `show_slot_2` |
| `3` | Show / hide slot 3 | `show_slot_3` |
| `4` | Show / hide slot 4 | `show_slot_4` |
| `5` | Show / hide slot 5 | `show_slot_5` |
| `6` | Show / hide slot 6 | `show_slot_6` |
| `7` | Show / hide slot 7 | `show_slot_7` |
| `8` | Show / hide slot 8 | `show_slot_8` |
| `9` | Show / hide slot 9 | `show_slot_9` |
| `V` | Select next shown stored trace (then live) | `next_trace` |
| `Shift+V` | Select previous shown stored trace (then live) | `prev_trace` |
| `Alt+V` | Select next trace incl. hidden (then live) | `next_any_trace` |
| `Alt+Shift+V` | Select previous trace incl. hidden (then live) | `prev_any_trace` |
| `A` | Show / hide the selected stored trace | `toggle_trace` |
| `F2` | Rename the selected trace… | `trace_rename` |
| `Delete` | Delete selected trace… (asks first) | `trace_delete` |
| `Alt+↑` | Display offset +1 dB of the selected curve | `offset_up` |
| `Alt+↓` | Display offset −1 dB of the selected curve | `offset_down` |
| `Alt+Shift+↑` | Display offset +3 dB of the selected curve | `offset_up_coarse` |
| `Alt+Shift+↓` | Display offset −3 dB of the selected curve | `offset_down_coarse` |
| `Alt+Home` | Display offset of the selected curve back to 0 | `offset_clear` |
| `Shift+O` | Open audio session… | `session_open` |
| `Shift+S` | Sweep measurement: response and harmonic distortion… | `sweep_new` |
| `Shift+L` | Leq windows and limits of the SPL meter… | `leq_windows` |

#### Transfer function

| Keys | Command | `keys.toml` |
|---|---|---|
| `F` | Freeze / unfreeze selected measurement | `freeze` |
| `R` | Reset averaging of selected measurement | `reset_average` |
| `S` | Start / stop selected measurement | `start_stop` |
| `X` | Delay: find and insert first arrival | `insert_delay` |
| `Shift+X` | Delay: find and insert strongest peak | `insert_strongest` |
| `D` | Delay: type value (ms)… | `type_delay` |
| `Y` | Delay tracking on / off | `track_delay` |
| `Ctrl+,` | Delay of the measurement −1 sample (the curve moves at once) | `delay_down` |
| `Ctrl+.` | Delay of the measurement +1 sample (the curve moves at once) | `delay_up` |
| `Alt+,` | Delay of the measurement −0.1 sample | `delay_down_fine` |
| `Alt+.` | Delay of the measurement +0.1 sample | `delay_up_fine` |
| `U` | Invert polarity of selected trace (display) | `invert` |
| `J` | Type dB offset of selected trace… | `offset` |
| `,` | Nudge selected trace 0.1 ms earlier | `nudge_earlier` |
| `.` | Nudge selected trace 0.1 ms later | `nudge_later` |
| `E` | Make selected trace the phase reference | `phase_reference` |
| `Z` | Load a target curve file… | `target` |
| `Shift+I` | Show / hide IR pane | `toggle_ir` |
| `B` | Coherence mask: off → 0.3 → 0.5 → 0.7 → 0.9 | `coherence_mask` |
| `Shift+C` | Coherence: own pane / over magnitude | `coherence_placement` |
| `M` | Average shown stored traces (power) | `average` |
| `P` | Phase wrapped / unwrapped | `phase_unwrap` |
| `K` | Smoothing coarser (selected trace or pane's measurement) | `smooth_coarser` |
| `Shift+K` | Smoothing finer (selected trace or pane's measurement) | `smooth_finer` |
| `Shift+P` | Phase / group delay | `group_delay` |

#### Spectrum / RTA

| Keys | Command | `keys.toml` |
|---|---|---|
| `F` | Freeze / unfreeze selected measurement | `freeze` |
| `R` | Reset averaging of selected measurement | `reset_average` |
| `S` | Start / stop selected measurement | `start_stop` |
| `J` | Type dB offset of selected trace… | `offset` |
| `K` | Smoothing coarser (selected trace or pane's measurement) | `smooth_coarser` |
| `Shift+K` | Smoothing finer (selected trace or pane's measurement) | `smooth_finer` |
| `B` | RTA: bars / line | `spectrum_style` |
| `P` | Peak hold on / off | `peak_hold` |
| `G` | Spectrograph under the spectrum on / off | `spectrograph` |
| `Shift+G` | Spectrograph history: 10 → 30 → 60 → 120 s | `spectrograph_span` |

#### Impulse response

| Keys | Command | `keys.toml` |
|---|---|---|
| `Shift+I` | Show / hide IR pane | `toggle_ir` |
| `G` | IR: linear → log → ETC | `ir_mode` |

#### SPL

| Keys | Command | `keys.toml` |
|---|---|---|
| `R` | Reset averaging of selected measurement | `reset_average` |
| `S` | Start / stop selected measurement | `start_stop` |
| `G` | SPL: meter → Leq windows → meter + Leq | `spl_leq_view` |
| `B` | SPL Leq windows: columns / tiles | `spl_leq_style` |
| `Shift+B` | SPL Leq windows: history strip on / off | `spl_leq_history` |
| `Shift+R` | Start a new SPL log… | `spl_new_log` |
| `F` | SPL meter: time weighting Fast → Slow → Impulse | `spl_time_weighting` |
| `Z` | SPL meter: frequency weighting A → C → Z | `spl_weighting` |

#### Sweep / distortion

| Keys | Command | `keys.toml` |
|---|---|---|
| `G` | IR: linear → log → ETC | `ir_mode` |
| `U` | Distortion in dB re fundamental / percent | `distortion_unit` |
| `Shift+I` | Sweep: distortion / impulse response | `sweep_ir` |
| `Shift+W` | Hide the sweep / distortion pane | `hide_distortion` |

#### Command palette only (`Ctrl+K`)

| Command | `keys.toml` |
|---|---|
| Stimulus: type output channels… | `stimulus_outputs` |
| Stimulus: take over the lease from another client and arm | `stimulus_take_over` |
| Choose the measurement the focused pane shows… | `pane_measurement` |
| Move the selected trace to slot… (1 … 9, none frees its slot) | `trace_slot` |
| Export the selected trace (ac2 CSV) to a file… | `trace_export` |
| Deselect the stored trace: keys act on the live measurement again | `select_live` |
| Import a trace file (CSV / analyzer text)… | `import_trace` |
| Session: save (name or path)… | `session_save` |
| Session: load, disarmed (name or path)… | `session_load` |
| Reconnect to the daemon now | `reconnect` |
| Close audio session | `session_close` |
| Record: raw audio of every input on / off (stops by itself after 1 h) | `record` |
| Replay a recording as the session (name or path)… | `replay_recording` |
| New transfer measurement… | `meas_new_transfer` |
| New spectrum… | `meas_new_spectrum` |
| New RTA… | `meas_new_rta` |
| New SPL meter… | `meas_new_spl` |
| Delete selected measurement | `meas_delete` |
| Input setup: mic, mic curve and calibration of each input… | `input_setup` |
| Calibrations: mics, curves and sensitivity calibrations… | `calibrations` |
| Input setup: type mic names (3=M30, 4=ECM)… | `input_mics` |
| Mic curve: next curve on the selected measurement's input (off → 0° → 90° …) | `mic_curve` |
| Mic curve on input N… (e.g. 2=90°, 2=off) | `mic_curve_input` |
| Calibration: delete a sensitivity calibration (input=mic)… | `cal_delete` |
| Mic curve on the selected trace (e.g. MM1 34804 90°; none removes)… | `trace_mic_curve` |
| SPL meter: Fast time weighting (125 ms) | `spl_fast` |
| SPL meter: Slow time weighting (1 s) | `spl_slow` |
| SPL meter: Impulse time weighting (35 ms / 1.5 s) | `spl_impulse` |
| SPL meter: A weighting | `spl_a` |
| SPL meter: C weighting | `spl_c` |
| SPL meter: Z weighting (flat) | `spl_z` |
| Delay finder: auto band (full → mid → sub) | `finder_auto` |
| Delay finder: full band (2–16 kHz) | `finder_full` |
| Delay finder: mid band (300 Hz – 3 kHz) | `finder_mid` |
| Delay finder: sub band (20–120 Hz) | `finder_sub` |
| Delay finder: custom band (Hz)… | `finder_custom` |
| Delay finder: observation length (s)… | `finder_observation` |
| Average shown stored traces (complex) | `average_complex` |
| Average shown stored traces (coherence-weighted) | `average_coherence` |
| A − B: dB difference of the two lowest shown slots | `math_difference` |
| A / B: complex division of the two lowest shown slots | `math_divide` |
| Smoothing: off | `smooth_off` |
| Smoothing: 1/48 oct | `smooth_48` |
| Smoothing: 1/24 oct | `smooth_24` |
| Smoothing: 1/12 oct | `smooth_12` |
| Smoothing: 1/6 oct | `smooth_6` |
| Smoothing: 1/3 oct | `smooth_3` |
| SPL pane: the meter | `spl_show_meter` |
| SPL pane: the Leq windows | `spl_show_leq` |
| SPL pane: meter + Leq windows | `spl_show_meter_leq` |

#### Key hint lines (`Shift+H` on / off)

The least used go first on a narrow pane; the sweep pane shows `U` while it shows distortion and `G` while it shows the impulse response.

| Pane | Hint line |
|---|---|
| Transfer function | `V` select trace · `A` show/hide · `Ctrl+1` capture · `X` find delay · `K` smoothing · `Shift+I` IR · `W` maximise · `Alt+↑` offset · `H` all keys |
| Spectrum / RTA | `S` start/stop · `F` freeze · `P` peak hold · `G` spectrograph · `K` smoothing · `Shift+Home` fit level · `Ctrl+1` capture · `W` maximise · `H` all keys |
| Impulse response | `G` linear/log/ETC · `N` next measurement · `Shift+I` hide pane · `W` maximise · `H` all keys |
| SPL | `G` meter/Leq/both · `F` F/S/I · `Z` A/C/Z · `B` columns/tiles · `Shift+B` history · `Shift+L` windows · `Shift+R` new log · `W` maximise · `H` all keys |
| Sweep / distortion | `Shift+S` new sweep · `N` next sweep · `U` dB/% · `G` linear/log/ETC · `Shift+I` IR/distortion · `W` maximise · `Shift+W` hide pane · `H` all keys |

<!-- keymap:end -->

## Command line

Every command takes `--json` for machine-readable output and `--remote <host>` to talk to a
network daemon. Live views (`--watch`) redraw in the terminal. Units are written with
suffixes: `-20dbfs`, `48khz`, `12.5ms`, `600samples`, `4.3m`, `94db`. `ac2 <command> --help`
documents each command; `ac2 discover` lists daemons on the local network.

| command | what it does |
|---|---|
| `ac2 devices`, `ac2 status`, `ac2 daemon start / stop / status` | the daemon and its audio devices; `status` includes the autosave state |
| `ac2 session open / close / status / inputs / save / load / list` | the audio session, each input's mic and active curve, saved sessions |
| `ac2 meas new / list / start / stop / rm` | transfer (`tf`), `spectrum`, `rta` and `spl` measurements |
| `ac2 gen pink / white / periodic-pink / sine`, `ac2 gen stop` | the generator in the foreground (Enter fires, Esc stops); `stop` from any client |
| `ac2 delay find / insert / set / nudge / track` | the delay finder and delay of a transfer measurement |
| `ac2 ir capture` | a sweep: response, distortion and impulse response, stored as a trace |
| `ac2 trace capture / list / show / rename / display / slot / rm / average / math / import / export / smooth / mic` | stored traces |
| `ac2 cal spl / electrical / curve import / curve rename / curve rm / use / list / rm` | sensitivity calibrations and the mic library |
| `ac2 spl watch`, `ac2 spl set`, `ac2 spl cal`, `ac2 spl leq watch / set / export / new` | SPL readout, the meter's weightings, acoustic calibration (as `cal spl`), Leq windows and presets, the per-second log and a new log |
| `ac2 timing --watch` | the loopback timing monitor |
| `ac2 state dump` | the daemon's whole state as JSON |
| `ac2 discover`, `ac2 auth pair / show` | find network daemons, pair with one |
