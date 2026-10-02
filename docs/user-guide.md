# ac2 user guide

ac2 is a live dual-channel FFT analyzer for tuning sound systems: transfer functions with
coherence, a delay finder, spectrum and RTA, a calibrated SPL meter, stored traces and
sessions. This guide explains the concepts and the everyday workflow. Installing:
[install.md](install.md). Scripting and integrations: [protocol.md](protocol.md), the
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
ac2 session open --backend cpal --device "<name>" --in 1-4 --rate 48khz
ac2 meas new tf --ref 1 --meas 2 --name main-l
ac2 meas new tf --ref 1 --meas 3 --name sub
ac2 meas new rta --input 2 --name rta
ac2 meas start main-l
```

The backend is always named: `cpal` (the OS audio host: ALSA, CoreAudio, WASAPI), `jack`, or
`fake`. The fake backend is a simulated rig for trying things out and is never chosen for you:
the session dialog preselects a real interface whenever the daemon lists one. Choosing
*Simulated rig* in the app's connect dialog starts it ready to measure (session open, a
transfer measurement "demo" running); *This computer's audio* starts with no session and
opens the session dialog.

### The session dialog

Nothing in the dialog is a channel number to type. From the top:

- **Backend** (**←/→**): every backend the daemon offers, with what it is; one it cannot use
  now says why (*JACK server not running*). A daemon started on real audio offers JACK and
  the system's audio; the simulated rig only appears on a daemon started on it.
- **Device** (**←/→**): its name and *N in / M out · rate · buffer*.
- **Channels**: one row per input and output, named by the backend where it can (JACK port
  names; system audio has none, so *Input 3*), and for inputs a **live meter** (RMS bar,
  peak tick, *CLIP* held for a second). The meters run before the session opens: the daemon
  opens the device for capture only — no output stream exists — so you can find the mic by
  tapping it. While a session is open on that device the dialog shows the session's own
  meters instead.
- **Roles**: **R** Reference (the loopback return, one input), **M** measurement mic (any
  number; **N** names one — the name is the mic's identity for calibrations), **S** Stimulus
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
  creates *Reference → <mic>* transfer measurements. **Esc** closes the dialog — and, as
  everywhere, stops the stimulus.

The measurement dialogs pick inputs the same way: by name, with their meters, **←/→**.

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
- **arm** (`Space`) and **fire** (`Enter`) are separate steps; **Esc** always stops, from any
  client, without needing anything else;
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
- **D** types a delay (`12.5ms`, `600samples`, or a distance such as `4.3m`, converted with
  the speed of sound at the set temperature); **,** and **.** nudge by 0.1 ms; **Y** tracks
  the delay continuously.
- From the CLI: `ac2 delay find main-l --insert`, or limit the band:
  `ac2 delay find sub --band 40hz-120hz`.

The inserted delay is also the time origin of the impulse-response pane (**H**).

## Traces and slots

A **trace** is a stored snapshot of a measurement's live result, with the metadata needed to
interpret it later: delay, polarity, offset, smoothing, calibration state, mic and time.

- **Ctrl+1 … Ctrl+9** capture the selected measurement into slot 1–9 (replacing what was
  there); **1 … 9** show and hide a slot.
- Overlays are drawn relative to the selected trace's measured delay, so relative arrival
  times between traces stay visible. **E** makes the selected trace the phase reference.
- **C** turns on the comparison cursor, synchronised across panes and traces; **Shift+←/→**
  moves it.
- **M** averages the shown stored traces (power; complex and coherence-weighted averages are
  in the command palette); A − B is a dB difference, A / B a complex division.
- **Z** loads a target curve; the command palette imports CSV and other analyzers' text
  exports. `ac2 trace export <name> --csv out.csv` exports.

## Sessions

`ac2 session save <name>` (or **Session: save** in the palette) stores the measurements and
traces, including slots and display edits, in the daemon's session directory;
`ac2 session load <name>` restores them. A loaded session always comes up disarmed: nothing
plays until someone types a level and fires.

## Calibration and SPL

Inputs carry a **mic name** (**N** on the input in the session dialog,
`ac2 session open … --mic 3=M30`, or **Input setup** in the palette). Calibrations are stored per device, input channel and mic, so moving a mic to
another input, or plugging in another mic, is noticed:

- **Sensitivity**: put a 94 dB (or 114 dB) acoustic calibrator on the mic and run
  `ac2 cal spl --input 3 --ref 94db`. SPL is then computed from the raw input level with that
  sensitivity.
- **Mic curve**: `ac2 cal mic-curve --input 3 <file.frd>` imports the mic's magnitude
  response, which is then corrected on that input (switchable per measurement with the mic
  curve command).
- A calibration from another mic or input is shown as such; otherwise its age is shown.
  `ac2 cal list` lists everything.

The **SPL meter** shows Fast / Slow / Impulse levels with A, C or Z weighting, Leq, LAeq,
LCeq, LCpeak, Lmax and Lmin, as a big-number display in the SPL pane or in the terminal:
`ac2 spl watch --input 3 --weight a` (add `--json` for one JSON line per update).

## Keyboard

Everything in the app is reachable from the keyboard. **/** (or **F1**) shows the bindings,
**Ctrl+K** opens the command palette, which finds every command by name and shows its key.
Keys are scoped: the focused pane's keys apply first, then the global ones. The defaults
avoid `[ ] + - =` and other keys that need AltGr or a dead key on Nordic and other European
layouts. The stimulus cluster is fixed: **Space** arm, **Enter** fire, **Esc** stop, **↑/↓**
level (±1 dB, with Shift ±3 dB).

Change bindings in `keys.toml` in the ac2 config directory (`~/.config/ac2` on Linux,
`~/Library/Application Support/ac2` on macOS, `%APPDATA%\ac2\config` on Windows):

```toml
[global]
cycle_theme = "Ctrl+T"

[transfer]
insert_delay = ["X", "Alt+D"]
```

### Keyboard map

<!-- keymap:begin (generated by crates/ac2-ui/tests/keymap_doc.rs) -->
Keys as on Linux and Windows; on macOS `Ctrl` is `⌘` and `Alt` is `⌥`. Every binding can be changed in `keys.toml` using the names in the last column.

#### Everywhere

| Keys | Command | `keys.toml` |
|---|---|---|
| `/` or `F1` | Show / hide key bindings | `help` |
| `Ctrl+K` | Command palette | `palette` |
| `Ctrl+Q` | Quit | `quit` |
| `Space` | Stimulus: arm (needs a typed level) | `stimulus_arm` |
| `Enter` | Stimulus: fire (when armed) | `stimulus_fire` |
| `Esc` | Stimulus: stop and disarm | `stimulus_stop` |
| `↑` | Stimulus level +1 dB | `level_up` |
| `↓` | Stimulus level −1 dB | `level_down` |
| `Shift+↑` | Stimulus level +3 dB | `level_up_coarse` |
| `Shift+↓` | Stimulus level −3 dB | `level_down_coarse` |
| `L` | Stimulus: type level (dBFS)… | `stimulus_level` |
| `Alt+1` | Focus transfer-function pane | `focus_transfer` |
| `Alt+2` | Focus spectrum / RTA pane | `focus_spectrum` |
| `Alt+3` | Focus impulse-response pane | `focus_ir` |
| `Alt+4` | Focus SPL pane | `focus_spl` |
| `Tab` | Focus next pane | `next_pane` |
| `Shift+Tab` | Focus previous pane | `prev_pane` |
| `W` | Focused pane only / split layout | `maximize_pane` |
| `N` | Select next measurement | `next_measurement` |
| `Shift+N` | Select previous measurement | `prev_measurement` |
| `T` | Theme: dark → light → high contrast | `cycle_theme` |
| `I` | Zoom frequency in | `zoom_in` |
| `O` | Zoom frequency out | `zoom_out` |
| `←` | Pan frequency down | `pan_left` |
| `→` | Pan frequency up | `pan_right` |
| `Home` | Reset zoom (20 Hz – 20 kHz) | `reset_view` |
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
| `Shift+O` | Open audio session… | `session_open` |

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
| `U` | Invert polarity of selected trace (display) | `invert` |
| `J` | Type dB offset of selected trace… | `offset` |
| `,` | Nudge selected trace 0.1 ms earlier | `nudge_earlier` |
| `.` | Nudge selected trace 0.1 ms later | `nudge_later` |
| `E` | Make selected trace the phase reference | `phase_reference` |
| `Z` | Load a target curve file… | `target` |
| `H` | Show / hide IR pane | `toggle_ir` |
| `B` | Coherence mask: off → 0.3 → 0.5 → 0.7 → 0.9 | `coherence_mask` |
| `Shift+C` | Coherence: own pane / over magnitude | `coherence_placement` |
| `M` | Average shown stored traces (power) | `average` |
| `P` | Phase wrapped / unwrapped | `phase_unwrap` |
| `Shift+P` | Phase / group delay | `group_delay` |

#### Spectrum / RTA

| Keys | Command | `keys.toml` |
|---|---|---|
| `F` | Freeze / unfreeze selected measurement | `freeze` |
| `R` | Reset averaging of selected measurement | `reset_average` |
| `S` | Start / stop selected measurement | `start_stop` |
| `B` | RTA: bars / line | `spectrum_style` |
| `H` | Peak hold on / off | `peak_hold` |

#### Impulse response

| Keys | Command | `keys.toml` |
|---|---|---|
| `H` | Show / hide IR pane | `toggle_ir` |
| `G` | IR: linear → log → ETC | `ir_mode` |

#### SPL

| Keys | Command | `keys.toml` |
|---|---|---|
| `R` | Reset averaging of selected measurement | `reset_average` |
| `S` | Start / stop selected measurement | `start_stop` |

#### Command palette only (`Ctrl+K`)

| Command | `keys.toml` |
|---|---|
| Stimulus: type output channels… | `stimulus_outputs` |
| Stimulus: take over the lease from another client and arm | `stimulus_take_over` |
| Import a trace file (CSV / analyzer text)… | `import_trace` |
| Session: save (name or path)… | `session_save` |
| Session: load, disarmed (name or path)… | `session_load` |
| Reconnect to the daemon now | `reconnect` |
| Close audio session | `session_close` |
| New transfer measurement… | `meas_new_transfer` |
| New spectrum… | `meas_new_spectrum` |
| New RTA… | `meas_new_rta` |
| New SPL meter… | `meas_new_spl` |
| Delete selected measurement | `meas_delete` |
| Input setup: type mic names (3=M30, 4=ECM)… | `input_mics` |
| Mic curve on / off for the selected measurement's input | `mic_curve` |
| Calibration: delete sensitivity and mic curve (input=mic)… | `cal_delete` |
| Calibration: delete sensitivity only (input=mic)… | `cal_delete_sensitivity` |
| Calibration: delete mic curve only (input=mic)… | `cal_delete_curve` |
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

<!-- keymap:end -->

## Command line

Every command takes `--json` for machine-readable output and `--remote <host>` to talk to a
network daemon. Live views (`--watch`) redraw in the terminal. Units are written with
suffixes: `-20dbfs`, `48khz`, `12.5ms`, `600samples`, `4.3m`, `94db`. `ac2 <command> --help`
documents each command; `ac2 discover` lists daemons on the local network.
