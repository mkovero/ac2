# ac2 on Windows — testing with an audio interface

This guide takes a tester from a fresh install to a real measurement on Windows with an
audio interface, in about an hour of hands-on time plus a one-hour unattended run. It covers
what is not verified on Windows yet: WASAPI capture and playback with a real interface,
timing, and long-run stability. The MSI install and the simulated rig were confirmed in a
Windows 11 VM on 2026-10-03.

## 1. Get and install the build

The build is an x64 installer for Windows 10 (1809) or newer, unsigned for now.

1. The installer `ac2-0.0.0-dev.10-windows-x64.msi` (built 2026-10-08; the code of main
   81fcb14, build id e415ef1, the same build as the macOS dev.10 disk image) is in this
   directory, with `SHA256SUMS` beside it. Check it in PowerShell with
   `Get-FileHash ac2-0.0.0-dev.10-windows-x64.msi` and compare with the line in
   `SHA256SUMS`. The `.zip` beside it holds the same three programs unpacked, for use
   without installing.
2. Run the .msi. SmartScreen shows *Windows protected your PC* (unknown publisher): choose
   **More info → Run anyway**. It installs into `C:\Program Files\ac2`, adds a Start-menu
   entry **ac2**, and puts that folder on the system `PATH`. A newer MSI replaces an older
   one; uninstall from **Settings → Apps**.
3. Microphone privacy: **Settings → Privacy & security → Microphone** must allow
   *Let desktop apps access your microphone*. Without it Windows hands ac2 silence from
   every input, including line inputs. ac2 reads your interface's inputs and nothing else.
4. Firewall: on this computer the daemon listens on `127.0.0.1:47820` and `:47821` only
   (loopback), so no firewall prompt is expected. If one appears anyway, note it in the
   report; denying it does not affect local use.

Where ac2 keeps its files: settings, calibrations and keys in `%APPDATA%\ac2\config`;
autosave, saved sessions and recordings in `%APPDATA%\ac2\data`.

For the tests below, run the daemon in its own window so its log is kept (an app started
from the Start menu with no daemon running hosts one inside itself, which writes no log).
Open a **new** Command Prompt (so the `PATH` from the installer applies) and run:

```bat
ac2 --version
set RUST_LOG=info
ac2d 2> "%USERPROFILE%\ac2-test.log"
```

`ac2 --version` should name e415ef1. Leave that window open (closing it stops the daemon;
**Ctrl+C** there, or `ac2 daemon stop` from another window, stops it cleanly). Then start
**ac2** from the Start menu: it connects to that daemon by itself.

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

ac2 uses WASAPI in shared mode through the Windows audio engine; there is no ASIO support.
Know this when reading the results:

- **One endpoint per direction.** Windows lists an interface's inputs and outputs as
  separate devices (often *Microphone (…)* or *Line (…)* and *Speakers (…)*; some drivers
  split them further into stereo pairs). The number of channels ac2 sees is what the
  Windows format of that endpoint says, not necessarily every input on the box.
- **Rate.** With no rate chosen, ac2 opens at the input device's Windows default format. A
  rate the device does not offer is refused with a message naming the rates it does offer.
- **Buffer.** With no buffer chosen, ac2 asks for about 20 ms (1024 frames at 48 kHz,
  within what the device allows). Below 256 frames WASAPI duplex is not expected to be
  reliable.
- **Clock.** Because input and output are separate endpoints, ac2 cannot prove they share
  a clock and says so in Settings (*separate endpoints … may run on different clocks*).
  That note is expected; the loopback reference keeps the transfer function correct
  either way.
- **Playing from ac2.** Windows lists the interface as two endpoints: a capture one (its
  inputs) and a playback one (its outputs). Settings › Audio has an **Input** and an
  **Output** device row; with a capture endpoint as the input, the output starts on the
  system's default output. Test 1 sets it to the interface's playback endpoint.

## 3. Test 1: the interface opens and its inputs meter

Pass: the interface is listed, its inputs show live levels, and a session opens without
errors.

1. In the app, **Shift+O** (or **Ctrl+K → Open audio session…**) opens Settings on its
   **Audio** page.
2. **Input** (←/→): step through the list and write down every entry for your interface
   and what each shows (*N in / M out · rate · buffer*). Pick the one with its inputs.
   **Output** (↓, then ←/→): pick the interface's playback endpoint (*0 in / N out*), not
   the laptop speakers. Note the clock note shown under the rows.
3. Go to the **Inputs & outputs** page (**Alt+1**). Tap the mic or play something into an
   input: its row's meter must move. Check every input you have. The output rows below
   are the playback endpoint's outputs.
4. Roles: **↑/↓** to a row, then **R** on input 1 (the loopback), **M** on input 2
   (cable 2; **N** names it, e.g. "cable"). **S** on output 1, and make sure output 2 is in
   the session too (**Space** on its row); optional: **D** (*Detect loopback…*) at −40 dBFS
   plays on the playback endpoint and should mark input 1 as the Reference by itself.
5. **Enter** opens the session. Accept the offered transfer measurement with **Enter**.
6. Check the top bar: device, rate and buffer are shown. The **Inputs** list on the left
   keeps a meter per input.

**Stimulus for the tests below.** ac2 plays its own pink noise on the playback endpoint (as
written in Test 2). If no output rows are listed, the Output row is not on the interface's
playback endpoint: go back to step 2.

From the CLI the same session is `ac2 session open --backend cpal --device "<capture
endpoint>" --out-device "<playback endpoint>" --in 1-2` (names as `ac2 devices` lists them).

Also worth a try: close the session (**Ctrl+K → Close audio session**), set both endpoints
to 96000 Hz in `mmsys.cpl`, reopen, and check the top bar shows 96 kHz. Unplug and replug
the interface while a session is open (ac2 should say so and recover, not crash). If the
interface does not open at all, copy the exact message from the app and the last lines of
`ac2-test.log`, and say which entry from step 2 you picked.

## 4. Test 2: transfer function over a cable, then the delay finder

Pass: an electrical loop reads flat (0 dB, 0°, coherence 1) and the delay finder reports
the interface's own latency between the two inputs, about 0 ms.

1. Make the stimulus play on both outputs: **Ctrl+P**, **Alt+1**
   (Settings › Inputs & outputs), **S** on output 2 as well, **Esc**. Output 1 feeds the
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
4. At the start and the end, note CPU % and memory of **ac2-ui.exe** and **ac2d.exe** in
   **Task Manager → Details**, and the battery percentage.
5. Afterwards, check the log for discontinuities, dropouts or bursts, in PowerShell:
   `Select-String -Path $HOME\ac2-test.log -Pattern 'discontinu|overflow|underrun|burst|xrun'`.
6. Minimise ac2 for a few minutes during the run, then bring it back. It should come back
   at once, not freeze while catching up.

## 7. Optional: record and replay

Checks the file writes and the replay path on Windows.

1. With the session open and pink noise playing, **Ctrl+K**, type `rec`, **Enter** on
   *Record: raw audio of every input on / off*. The top bar shows `REC 0:12 · … MB`. After
   a minute, the same command again stops it; the top bar names the recording
   (`recorded rec-… · 1:00 · … MB`).
2. Stop the noise. **Ctrl+K → Replay a recording as the session (name or path)…**, type
   that name, **Enter**. The measurements should show the same curves as live. Note what
   the app shows when the recording ends.
3. The files are in `%APPDATA%\ac2\data\recordings\`. Note their size.

## 8. What to send back

A short note per test is enough; failures with a screenshot and the log are the most
useful.

| Item | Your result |
| --- | --- |
| PC model, CPU, GPU, VM or not; Windows version and build (`winver`) | |
| Interface model, driver (maker's or class driver) and version; Windows format set | |
| `ac2 --version` output (and the top bar's *build …*) | |
| Test 1: device list entries for the interface; input meters move; Output set to the playback endpoint, its rows listed; clock note; session opens (rate / buffer shown) | |
| Output device chosen (name as listed); detect loopback found input 1? | |
| Test 2: cables read 0 dB / 0° / coherence 1; delay finder result (ms, confidence); banners | |
| Test 2 sweep (if ac2 plays): flat response, distortion level, arrival; finished cleanly? | |
| Test 3: spectrum, RTA and SPL meter live; calibration if tried | |
| Test 4: 1 h run clean? CPU %, memory (both processes) and battery at start and end | |
| Record and replay (optional): file written, replay matches live? | |
| Do the input meters and the SPL number move smoothly, or noticeably steppy? | |
| CPU % of ac2-ui.exe and ac2d.exe with a session open and nothing playing | |
| Any banner, freeze, crash, firewall prompt or odd behaviour | |

Attach `%USERPROFILE%\ac2-test.log` and screenshots of anything that looked wrong.
