# ac2 on Windows — testing with an audio interface

This guide takes a tester from a fresh install to a real measurement on Windows with an
audio interface, in about an hour of hands-on time plus a one-hour unattended run. It covers
what is not verified on Windows yet: capture and playback with a real interface over ASIO,
timing, and long-run stability. On Windows ac2 talks to audio through ASIO only: the
interface's own ASIO driver, or FlexASIO for a device without one (§2). The MSI install
and the simulated rig were confirmed in a Windows 11 VM on 2026-10-03. CI (all-OS and
release runs) opens ASIO through FlexASIO on a virtual cable, VB-CABLE
(`tools/release/windows-asio.ps1`), and the app comes up on it. A dispatch can also run
that session for an hour (`soak_minutes`). None of this substitutes for an interface.

## 1. Get and install the build

The build is an x64 installer for Windows 10 (1809) or newer, unsigned for now. The dev.10
build below predates ASIO and runs the older system-audio backend; the steps in §2 onwards
need a newer build, which lists the backend **ASIO**.

1. The installer `ac2-0.0.0-dev.10-windows-x64.msi` (built 2026-10-08; the code of main
   81fcb14, build id e415ef1, the same build as the macOS dev.10 disk image) is in this
   directory, with `SHA256SUMS` beside it. Check it in PowerShell with
   `Get-FileHash ac2-0.0.0-dev.10-windows-x64.msi` and compare with the line in
   `SHA256SUMS`. The `.zip` beside it holds the same three programs unpacked, for use
   without installing.
   Release files are not in git: if this directory has only `SHA256SUMS`, fetch them from
   the dev.10 release run with `gh run download 37819512973 -R mkovero/ac2 -n dist-windows`
   (or the run's *Artifacts* on GitHub; kept until 2027-01-06).
2. Run the .msi. SmartScreen shows *Windows protected your PC* (unknown publisher): choose
   **More info → Run anyway**. It installs into `C:\Program Files\ac2`, adds a Start-menu
   entry **ac2**, and puts that folder on the system `PATH`. A newer MSI replaces an older
   one; uninstall from **Settings → Apps**.
3. Microphone privacy, when the device runs through FlexASIO (§2): **Settings → Privacy &
   security → Microphone** must allow *Let desktop apps access your microphone*. FlexASIO
   opens the device through Windows' own audio stack (WASAPI), and without that switch
   Windows refuses the input ("Access is denied", or FlexASIO fails to start) or hands ac2
   silence from every input, including line inputs. A maker's ASIO driver bypasses it. ac2
   reads your interface's inputs and nothing else.
4. Firewall: on this computer the daemon listens on `127.0.0.1:47820` and `:47821` only
   (loopback), so no firewall prompt is expected. If one appears anyway, note it in the
   report; denying it does not affect local use.

Where ac2 keeps its files: settings, calibrations and keys in `%APPDATA%\ac2\config`;
autosave, saved sessions and recordings in `%APPDATA%\ac2\data`; logs in
`%LOCALAPPDATA%\ac2\logs`.

`ac2 --version` in a **new** Command Prompt (so the `PATH` from the installer applies) should
name e415ef1. Then start **ac2** from the Start menu. It keeps its log, including the audio
engine it hosts, in `%LOCALAPPDATA%\ac2\logs\ac2-ui.log` (paste that into Explorer's
address bar); the previous run's is `ac2-ui.log.1`. Each start replaces the older one, so
copy the file before starting ac2 again if a run went wrong.

## 2. Wiring and safety

The core tests use two cables and no speaker, so every expected result is known; a speaker
and a mic are optional extras.

```
interface out 1 ── cable 1 ──► in 1   (reference: the loopback)
interface out 2 ── cable 2 ──► in 2   (measurement: here a wire, so the answer is known)
```

- **Loopback:** cable 1 from output 1 to input 1, cable 2 from output 2 to input 2 (line
  level; turn both inputs' preamp gain to minimum, pads on if they have them).
- **No speaker connected for the first tests.** If one is connected, keep the amp or
  monitor level low.
- ac2 never plays anything without a typed level: **L** types it, **Space** arms,
  **Enter** plays, **Shift+Esc** stops from anywhere. Start at **−40 dBFS** and stay at or
  below **−20 dBFS** unless you know your gain structure. If you play noise from another
  program instead (see Test 1), set the Windows volume of that output low first and keep
  the noise file itself quiet (about −40 dBFS).
- Windows side, for the interface's input and its output: in the Sound control panel
  (**Win+R**, `mmsys.cpl`), on the **Recording** and **Playback** tabs, open the device's
  **Properties → Advanced**. Set **Default Format** to the same rate on both (48000 Hz to
  start), and turn off **audio enhancements** where the page offers them. Note what you
  set.
- Note your interface model, its driver version (and whether you use the maker's driver or
  the Windows class driver), the Windows version and build (**Win+R**, `winver`), the CPU
  and GPU, and whether this is a VM. These go in the report.

### How ac2 talks to the interface on Windows

ASIO is ac2's only Windows audio path: one driver serves the interface's inputs and outputs
from one callback, on one clock, with the channel names the driver gives (*Analog 1*,
*ADAT 3*). The session dialog and `ac2 devices` list the backend **ASIO**, one device per
installed driver.

#### The driver

Install your interface maker's ASIO driver. With no ASIO driver installed at all, the
session dialog and `ac2 devices` show the backend as unavailable with *No ASIO driver
found*, naming FlexASIO.

#### FlexASIO: devices without their own ASIO driver

A USB measurement mic, a class-compliant interface or a laptop's built-in audio has no
ASIO driver of its own. It works through [FlexASIO](https://github.com/dechamps/FlexASIO)
(free, MIT), an ASIO driver over Windows' own audio stack:

1. Install FlexASIO. It appears in ac2 as the device **FlexASIO**.
2. Point it at your device with a text file `%USERPROFILE%\FlexASIO.toml`, for example

   ```toml
   backend = "Windows WASAPI"

   [input]
   device = "Microphone (USB Audio CODEC)"

   [output]
   device = "Speakers (USB Audio CODEC)"
   ```

   using the device names as **Sound** settings (`mmsys.cpl`) show them. Input and output
   may be different devices (a USB mic in, the interface out); ac2 then still sees one ASIO
   device, but the two may run on different clocks, so keep the loopback reference.
   FlexASIO reads the file whenever the driver loads: close the session (or quit the app)
   after editing it.
3. Allow microphone access for desktop apps (§1 step 3): FlexASIO opens the input through
   WASAPI, which applies Windows' microphone privacy switch.

FlexASIO's timing is WASAPI shared mode's underneath, not a pro driver's; report which
driver you used.

#### Using ASIO

- **In the app.** The session dialog's backend row shows **ASIO**; each installed ASIO
  driver is one device, with inputs and outputs together.
- **From the command line.** `ac2 devices` lists the drivers under `asio`, with channel
  names. `ac2 session open --backend asio --device "<driver>" --in 1-2` opens one;
  `ac2 selftest duplex --backend asio --device "<driver>" --duration 60s` is Test 5 on it
  (close the app first: a driver runs in one program at a time).
- **Rate and buffer.** With none chosen, ac2 opens at the driver's current rate and its
  preferred buffer size. Many drivers set the buffer only in their own control panel: a
  change there (or a rate change on the interface) ends the stream, and ac2 reopens it by
  itself within a few seconds (the top bar shows the outage). Report how long it took.
- **Clock.** Input and output come from the same driver callback, so Settings shows one
  clock; there is no *separate endpoints* note.
- **One driver at a time.** ASIO allows one driver per program. While ac2 has a driver
  open, the other drivers are listed by name only; ac2 cannot pair one driver's inputs with
  another's outputs.

## 3. Test 1: the interface opens and its inputs meter

Pass: the interface is listed, its inputs show live levels, and a session opens without
errors.

1. In the app, **Shift+O** (or **Ctrl+K → Open audio session…**) opens Settings on its
   **Audio & I/O** page, on the **Input** row.
2. **Input** (←/→): step through the list and write down every entry and what each shows
   (*N in / M out · rate · buffer*). Pick your interface's ASIO driver (or **FlexASIO**).
   The **Output** row follows it: one ASIO driver serves both directions. Note the clock
   note shown under the rows.
3. Go down (**↓**) to the input rows. Tap the mic or play something into an
   input: its row's meter must move. Check every input you have. The output rows below
   are the driver's outputs.
4. Roles: **↑/↓** to a row, then **R** on input 1 (the loopback), **M** on input 2
   (cable 2; **N** names it, e.g. "cable"). **S** on output 1, and make sure output 2 is in
   the session too (**Space** on its row); optional: **D** (*Detect loopback…*) at −40 dBFS
   plays on the driver's outputs and should mark input 1 as the Reference by itself.
5. **Enter** opens the session. Accept the offered transfer measurement with **Enter**.
6. Check the top bar: device, rate and buffer are shown. The **Inputs** list on the left
   keeps a meter per input.

**Stimulus for the tests below.** ac2 plays its own pink noise on the driver's outputs (as
written in Test 2). If no output rows are listed, the driver has no outputs (with FlexASIO,
check the `[output]` device in `FlexASIO.toml`): go back to step 2.

From the CLI the same session is `ac2 session open --backend asio --device "<driver>" --in
1-2` (the name as `ac2 devices` lists it).

Also worth a try: close the session (**Ctrl+K → Close audio session**), set the interface to
96000 Hz in its driver's control panel (with FlexASIO, both devices in `mmsys.cpl`), reopen,
and check the top bar shows 96 kHz. Unplug and replug
the interface while a session is open (ac2 should say so and recover, not crash). If the
interface does not open at all, copy the exact message from the app and the last lines of
`ac2-ui.log`, and say which entry from step 2 you picked.

## 4. Test 2: transfer function over a cable, then the delay finder

Pass: an electrical loop reads flat (0 dB, 0°, coherence 1) and the delay finder reports
the interface's own latency between the two inputs, about 0 ms.

1. Make the stimulus play on both outputs: **Ctrl+P**, **Alt+1**
   (Settings › Audio & I/O), **S** on output 2 as well, **Esc**. Output 1 feeds the
   reference over cable 1, output 2 the "measurement" over cable 2, so the measurement is a
   wire and the answer is known.
2. **L**, type `-40`, **Enter**; **Space** arms, **Enter** plays pink noise. The transfer pane should show magnitude near 0 dB, phase near 0° and
   coherence near 1 across 20 Hz–20 kHz.
3. Press **X** (delay finder). Expect a first arrival close to 0 ms, a high confidence,
   and no "no estimate".
4. Watch for **banners** over the plots (NO REFERENCE, NO SIGNAL, CLIP, STALE, or a
   loopback timing warning about dropped or repeated output samples or drift). With signal
   playing there should be none; note any that appear.
5. **Shift+Esc** stops. Take a screenshot (**Win+Shift+S**,
   or **Win+PrtScn**, which saves to *Pictures\Screenshots*) while playing.

With a speaker and mic instead of the second cable: same steps at a quiet level. The delay
should match the mic distance (about 2.9 ms per metre); magnitude shows the speaker and
room.

**Sweep over the cable** (only when ac2 plays itself; same wiring, stimulus stopped):

1. **Shift+S** opens *New sweep measurement*: reference input 1, mic input 2, output 2
   (output 1, the loopback, always plays too), level `-40`, 3 s, **Enter**. Nothing plays
   yet; the **Sweep / distortion** pane comes up.
2. **Space** arms, **Enter** plays. A strip at the bottom counts down, then *analysing…*.
3. Expect a flat response near 0 dB across the band and distortion far down (H2, H3 well
   below −80 dB, or drawn dashed at the noise floor). The arrival is about 0 ms.
4. **G** steps to the impulse response (one sharp peak) and the room table (mostly `—` over
   a cable: there is no decay). Note anything odd, and whether the run finished without a
   dropout warning.

## 5. Test 3: spectrum, RTA and the SPL meter

Pass: each view shows live data that moves with the signal, and the SPL meter reads
plausible levels.

1. **Ctrl+K → New spectrum…** on input 1, then **New RTA…** on input 1. With pink noise
   playing at −40 dBFS over the loopback, the RTA bands should sit roughly level with each
   other (pink noise has equal energy per octave) and the spectrum should slope down about
   3 dB per octave.
2. **Ctrl+K → New SPL meter…** on input 2. Uncalibrated it reads dBFS: with pink noise at
   −40 dBFS on cable 2 it should read close to −40 with Z weighting (**Z** in the SPL pane
   cycles A / C / Z; A reads lower on pink noise) and follow the level when you change it
   (with ↑/↓ when ac2 plays, or the player's volume). A mic on a spare input works too.
3. Optional, if a calibrator is at hand: calibrate the mic input (Settings › Calibration,
   **C** on the input, 94 or 114 dB) and check the meter then reads dB SPL.
4. Maximise the SPL pane (**W**); a new meter shows its Leq windows below the number
   (**G** steps the views). The 1/5/10/30/60 min windows should start filling.

## 6. Test 4: one-hour run, unattended

Pass: an hour of continuous measuring with no audio dropouts, no growing memory and no
crash.

1. Keep the transfer measurement, spectrum, RTA and SPL meter running, with pink noise at
   −40 dBFS over the cables only (no speaker).
2. On a laptop, run it on battery if you can; that is the case we care about most.
3. Leave it for one hour. Don't let the PC sleep: **Settings → System → Power** (*Power &
   battery* on Windows 11), screen and sleep to **Never** for the run.
4. At the start and the end, note CPU % and memory of **ac2-ui.exe** in
   **Task Manager → Details**, and the battery percentage.
5. Afterwards, check the log for discontinuities, dropouts or bursts, in PowerShell:
   `Select-String -Path $env:LOCALAPPDATA\ac2\logs\ac2-ui.log -Pattern 'discontinu|overflow|underrun|burst|xrun'`.
6. Minimise ac2 for a few minutes during the run, then bring it back. It should come back
   at once, not freeze while catching up.

CI's hour on FlexASIO and a virtual cable (`-f soak_minutes=60`, see
`docs/design/asio.md` §4b) runs the daemon alone. It has no interface clock, no vendor
driver, no app drawing and no battery. It is the closest thing without hardware, but it does
not replace this run.

## 7. Test 5: duplex self-test (command line)

Checks the interface below the app: the same audio backend, run for a fixed time, judged
pass or fail with a named reason per failure. Needs a build newer than dev.10 (`ac2 selftest
--help` answers if yours has it). Use `ac2` in a new Command Prompt (the installer put it on
the `PATH`). **Quit the ac2 app first** so the device is free.

1. **Silent** (no cable needed; the outputs open but play only zeros):

   ```
   ac2 selftest duplex --backend asio --device "<driver>" --duration 60s
   ```

   `<driver>` is your interface's ASIO driver (or `FlexASIO`) as the app lists it. A wrong
   name prints the drivers it knows. Without `--device` it tests the first installed
   driver. It
   reports device, channel counts, rate and buffer, block sizes, xruns and gaps, callback
   timing, both clocks against the computer's clock and the peak of every input, and ends in
   `result PASS` or one `FAIL` line per reason.

2. Same again while loading the computer (a browser benchmark, a big build): any dropout
   must show up as a `FAIL` line, not pass silently.

3. **With the loopback cable** (cable 1, output 1 → input 1; optional, plays pink noise on
   output 1 only, at the level you type; nothing but the cable on that output):

   ```
   ac2 selftest duplex --backend asio --device "<driver>" --duration 30s ^
       --emit -40dbfs --loopback-out 1 --loopback-in 1
   ```

   It refuses levels above −20 dBFS, fades in and out, and Ctrl-C fades out early. It adds
   the output→input delay (`loopback offset`), whether it held still, and the drift between
   output and input clocks. Run it about five times and note each offset: whether it changes
   between runs is the point.

A `--rate` the driver does not offer should fail to open, by name; `--buffer 128samples` is
worth one silent run too (many drivers take the buffer only from their control panel and
say so). Paste each report into what you send back
(`--json` gives the same as JSON).

## 8. Optional: record and replay

Checks the file writes and the replay path on Windows.

1. With the session open and pink noise playing, **Ctrl+K**, type `rec`, **Enter** on
   *Record: raw audio of every input on / off*. The top bar shows `REC 0:12 · … MB`. After
   a minute, the same command again stops it; the top bar names the recording
   (`recorded rec-… · 1:00 · … MB`).
2. Stop the noise. **Ctrl+K → Replay a recording as the session (name or path)…**, type
   that name, **Enter**. The measurements should show the same curves as live. Note what
   the app shows when the recording ends.
3. The files are in `%APPDATA%\ac2\data\recordings\`. Note their size.

## 9. What to send back

A short note per test is enough; failures with a screenshot and the log are the most
useful.

| Item | Your result |
| --- | --- |
| PC model, CPU, GPU, VM or not; Windows version and build (`winver`) | |
| Interface model; ASIO driver (maker's or FlexASIO) and version; with FlexASIO, its `FlexASIO.toml` | |
| `ac2 --version` output (and the top bar's *build …*) | |
| Test 1: device list entries for the interface; input meters move; output rows listed; clock note; session opens (rate / buffer shown) | |
| Detect loopback found input 1? | |
| Test 2: cables read 0 dB / 0° / coherence 1; delay finder result (ms, confidence); banners | |
| Test 2 sweep (if ac2 plays): flat response, distortion level, arrival; finished cleanly? | |
| Test 3: spectrum, RTA and SPL meter live; calibration if tried | |
| Test 4: 1 h run clean? CPU %, memory (both processes) and battery at start and end | |
| Test 5: self-test reports (silent, under load, loopback offsets if run) | |
| Record and replay (optional): file written, replay matches live? | |
| Do the input meters and the SPL number move smoothly, or noticeably steppy? | |
| CPU % of ac2-ui.exe with a session open and nothing playing | |
| Any banner, freeze, crash, firewall prompt or odd behaviour | |

Attach `%LOCALAPPDATA%\ac2\logs\ac2-ui.log` (and `ac2-ui.log.1` if the run in question was
the one before) and screenshots of anything that looked wrong.
