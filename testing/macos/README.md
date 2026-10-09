# ac2 on macOS — testing with an audio interface

This guide takes a tester from a fresh install to a real measurement on macOS with an audio
interface, in about an hour of hands-on time plus a one-hour unattended run. It covers what
is not verified on macOS yet: Core Audio capture and playback, timing, and long-run
stability. Install and first start (microphone prompt) were confirmed on 2026-10-05.

## 1. Get and install the build

The build is a universal disk image (Apple silicon and Intel, macOS 11 or newer), unsigned
for now.

1. The disk image `ac2-0.0.0-dev.10-macos-universal.dmg` (built 2026-10-08;
   the code of main 81fcb14, build id e415ef1) is in this directory, with `SHA256SUMS` beside it (check with
   `shasum -a 256 -c SHA256SUMS --ignore-missing`). The `.zip` beside it holds the same
   programs unpacked, for use without the disk image.
   Release files are not in git: if this directory has only `SHA256SUMS`, fetch them from
   the dev.10 release run with `gh run download 37819512973 -R mkovero/ac2 -n dist-macos`
   (or the run's *Artifacts* on GitHub; kept until 2027-01-06).
2. Open the .dmg and drag **ac2** to Applications. If a previous ac2 is there, replace it.
3. First start: **right-click ac2.app in Applications → Open**, then **Open** in the
   dialog. macOS remembers the answer for this one app; there is no need to allow unsigned
   apps in general. On macOS 15, use **System Settings → Privacy & Security → Open Anyway**
   after a first refused start.
4. Allow microphone access when asked. ac2 reads your interface's inputs and nothing else.
5. Optional, for the command-line tools: copy them from the disk image with
   `sudo cp /Volumes/ac2*/bin/ac2 /Volumes/ac2*/bin/ac2d /usr/local/bin/` and run
   `xattr -d com.apple.quarantine /usr/local/bin/ac2 /usr/local/bin/ac2d`.

Start the app as usual (Launchpad, Finder or Dock). It keeps its log, including the audio
engine it hosts, in `~/Library/Logs/ac2/ac2-ui.log` (Console.app lists it under *Log
Reports*); the previous run's is `ac2-ui.log.1`. Each start replaces the older one, so copy
the file before starting ac2 again if a run went wrong.

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
  below **−20 dBFS** unless you know your gain structure.
- Note your interface model, its driver or firmware version, the macOS version, and
  whether the Mac is Apple silicon or Intel. These go in the report.

## 3. Test 1: the interface opens and its inputs meter

Pass: the interface is listed, its inputs show live levels, and a session opens without
errors.

1. Start ac2. In the connect dialog choose **This computer's
   audio**. Settings opens on its **Audio** page.
2. **Device** (←/→): pick your interface. Note what it shows (*N in / M out · rate ·
   buffer*).
3. Go to the **Inputs & outputs** page (**Alt+1**). Tap the mic or play something into an
   input: its row's meter must move. Check every input you have.
4. Roles: **↑/↓** to a row, then **R** on input 1 (the loopback), **M** on input 2
   (cable 2; **N** names it, e.g. "cable"), **S** on output 1, and make sure output 2 is in
   the session too (**Space** on its row). Optional: **D** (*Detect loopback…*) at −40 dBFS
   should mark input 1 as the Reference by itself.
5. **Enter** opens the session. Accept the offered transfer measurement with **Enter**.
6. Check the top bar: device, rate and buffer are shown. The **Inputs** list on the left
   keeps a meter per input.

Also worth a try: close the session (**Ctrl+K → Close audio session**), reopen it at
96 kHz if the interface supports it, and unplug and replug the interface while a session
is open (ac2 should say so and recover, not crash).

## 4. Test 2: transfer function over a cable, then the delay finder

Pass: an electrical loop reads flat (0 dB, 0°, coherence 1) and the delay finder reports
the interface's own latency between the two inputs, about 0 ms.

1. Make the stimulus play on both outputs: **Ctrl+P**, **Alt+1** (Settings › Inputs &
   outputs), **S** on output 2 as well, **Esc**. Output 1 feeds the reference over cable 1,
   output 2 the "measurement" over cable 2, so the measurement is a wire and the answer is
   known.
2. **L**, type `-40`, **Enter**; **Space** arms, **Enter** plays pink noise. The transfer
   pane should show magnitude near 0 dB, phase near 0° and coherence near 1 across
   20 Hz–20 kHz.
3. Press **X** (delay finder). Expect a first arrival close to 0 ms, a high confidence,
   and no "no estimate".
4. Watch for **banners** over the plots (NO REFERENCE, NO SIGNAL, CLIP, STALE, or a
   loopback timing warning about dropped or repeated output samples). With signal playing
   there should be none.
5. **Shift+Esc** stops. Take a screenshot (**⌘⇧3**) while playing.

With a speaker and mic instead of the second cable: same steps at a quiet level. The delay
should match the mic distance (about 2.9 ms per metre); magnitude shows the speaker and
room.

**Sweep over the cable** (same wiring, stimulus stopped):

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
   with ↑/↓. A mic on a spare input works too.
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
3. Leave it for one hour. Don't let the Mac sleep: **System Settings → Lock Screen**, or
   run `caffeinate -d` in a second Terminal window.
4. At the start and the end, note ac2's CPU % and memory in **Activity Monitor**, and the
   battery percentage.
5. Afterwards, check the log for discontinuities, dropouts or bursts:
   `grep -iE "discontinu|overflow|underrun|burst|xrun" ~/Library/Logs/ac2/ac2-ui.log`.
6. Minimise ac2 for a few minutes during the run, then bring it back. It should come back
   at once, not freeze while catching up.

## 7. Test 5: duplex self-test (command line)

Checks the interface below the app: the same audio backend, run for a fixed time, judged
pass or fail with a named reason per failure. Needs a build newer than dev.10 (`ac2 selftest
--help` answers if yours has it). Use the command-line tools from section 1, step 5, in
Terminal. The first run asks for microphone access for Terminal: allow it (a denial must end
in a clean `FAIL`, not a hang; worth noting). **Quit the ac2 app first** so the device is
free.

1. **Silent** (no cable needed; the outputs open but play only zeros):

   ```
   ac2 selftest duplex --backend cpal --device "<interface>" --duration 60s
   ```

   `<interface>` is your interface as the app lists it, e.g. `--device "Babyface Pro"`. A
   wrong name prints the devices it knows. Without `--device` it tests the system default
   input. It reports device, channel counts, rate and buffer, block sizes, xruns and gaps,
   callback timing, both clocks against the computer's clock and the peak of every input,
   and ends in `result PASS` or one `FAIL` line per reason.

2. Same again while loading the computer (a browser benchmark, a big build): any dropout
   must show up as a `FAIL` line, not pass silently.

3. **With the loopback cable** (cable 1, output 1 → input 1; optional, plays pink noise on
   output 1 only, at the level you type; nothing but the cable on that output):

   ```
   ac2 selftest duplex --backend cpal --device "<interface>" --duration 30s \
       --emit -40dbfs --loopback-out 1 --loopback-in 1
   ```

   It refuses levels above −20 dBFS, fades in and out, and Ctrl-C fades out early. It adds
   the output→input delay (`loopback offset`), whether it held still, and the drift between
   output and input clocks. Run it about five times and note each offset: whether it changes
   between runs is the point.

macOS: an aggregate device of two interfaces is worth one silent run too. Paste each report
into what you send back (`--json` gives the same as JSON).

## 8. Optional: record and replay

Checks the file writes and the replay path on macOS.

1. With the session open and pink noise playing, **Ctrl+K**, type `rec`, **Enter** on
   *Record: raw audio of every input on / off*. The top bar shows `REC 0:12 · … MB`. After
   a minute, the same command again stops it; the top bar names the recording
   (`recorded rec-… · 1:00 · … MB`).
2. **Shift+Esc** stops the stimulus. **Ctrl+K → Replay a recording as the session (name or
   path)…**, type that name, **Enter**. The measurements should show the same curves as
   live. Note what the app shows when the recording ends.
3. The files are in `~/Library/Application Support/ac2/recordings/`. Note their size.

## 9. What to send back

A short note per test is enough; failures with a screenshot and the log are the most
useful.

| Item | Your result |
| --- | --- |
| Mac model, Apple silicon or Intel, macOS version | |
| Interface model, driver or firmware version | |
| ac2 version (top bar, *build …*) | |
| Test 1: interface listed, input meters move, session opens (rate / buffer shown) | |
| Test 2: cables read 0 dB / 0° / coherence 1; delay finder result (ms, confidence) | |
| Test 2 sweep: flat response, distortion level, arrival; finished cleanly? | |
| Test 3: spectrum, RTA and SPL meter live; calibration if tried | |
| Test 4: 1 h run clean? CPU %, memory and battery at start and end | |
| Test 5: self-test reports (silent, under load, loopback offsets if run) | |
| Record and replay (optional): file written, replay matches live? | |
| Do the input meters and the SPL number move smoothly, or noticeably steppy? | |
| ac2 CPU % in Activity Monitor with a session open and nothing playing | |
| Any banner, freeze, crash or odd behaviour | |

Attach `~/Library/Logs/ac2/ac2-ui.log` (and `ac2-ui.log.1` if the run in question was the
one before) and screenshots of anything that looked wrong.
