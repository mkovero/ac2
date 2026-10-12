# ac2 on macOS — testing with an audio interface

This guide takes a tester from a fresh install to a real measurement on macOS with an audio
interface, in about an hour of hands-on time plus a one-hour unattended run. It covers what
is not verified on macOS yet: Core Audio capture and playback on a real interface, timing,
and long-run stability. Install and first start were confirmed on an earlier build; the
audio path has so far run only on a virtual device (BlackHole) in CI.

## What is new in this build

- **One audio backend: Core Audio.** ac2 talks to the Core Audio HAL directly (`coreaudio`:
  one callback per device, inputs and outputs on one exact sample index, the device's
  latency stated). It is the only backend in the app, `ac2d` and the self-test; the cpal
  backend of earlier builds is gone, and so is `--backend cpal`.
- **Device settings set again once.** What an earlier build remembered per device (stimulus
  outputs, roles, mic names in a session default) was kept under the old backend and is not
  found now: set it again the first time. Device ids (`coreaudio:<UID>`) are unchanged.
  Raw recordings made by an earlier build no longer open for replay.
- **Microphone consent.** ac2 asks macOS before it opens any input. The first start may
  ask again even if an earlier build was allowed (macOS may treat each unsigned build as a
  new app). If access is refused, ac2 says so in words (*macOS denies ac2 the
  microphone: allow it in System Settings › Privacy & Security › Microphone*) instead of
  hanging.
- **Audio MIDI Setup on P.** On a Core Audio device, **P** in Settings › Audio & I/O brings
  up macOS's Audio MIDI Setup, where the device's rate, format and clock source are set. A
  change there reopens the session at the new settings by itself.
- **Only the inputs in use are read.** On an interface with several input streams (analog
  and ADAT, say), only the streams the session's inputs are on are switched on.
- **BlackHole.** If BlackHole is installed it is listed like any device. It is what CI tests
  on; it reports no latency and cannot be unplugged, so it answers none of Test 6. Use the
  real interface for every test here.

## 1. Get and install the build

The build is a universal disk image (Apple silicon and Intel, macOS 11 or newer), unsigned
for now.

<!-- BUILD:begin: the maintainer fills in this block for each build -->
- Version **`<VERSION>`**, built `<BUILD_DATE>` from main `<COMMIT>` (release workflow run
  `<RUN_ID>`).
- Files: `ac2-<VERSION>-macos-universal.dmg`, and `ac2-<VERSION>-macos-universal.zip` with
  the same programs unpacked, for use without the disk image.
- Fetch them with
  `gh run download <RUN_ID> -R mkovero/ac2 -n dist-macos`
  (or the run's *Artifacts* on GitHub; kept until `<EXPIRES>`).
<!-- BUILD:end -->

1. Release files are not in git: if this directory has only `SHA256SUMS`, fetch them with
   the `gh run download` line above. Check them with
   `shasum -a 256 -c SHA256SUMS --ignore-missing`.
2. Open the .dmg and drag **ac2** to Applications. If a previous ac2 is there, replace it.
3. First start: **right-click ac2.app in Applications → Open**, then **Open** in the
   dialog. macOS remembers the answer for this one app; there is no need to allow unsigned
   apps in general. On macOS 15 the dialog has no Open button: after the first refused
   start, use **System Settings → Privacy & Security**, scroll down, **Open Anyway**.
4. Allow microphone access when asked (it comes when ac2 first opens an input, usually as
   Settings shows the input meters). ac2 reads your interface's inputs and nothing else.
   Note whether the prompt came, and when.
5. Optional, for the command-line tools (`ac2` and the daemon `ac2d`, in the disk image's
   `bin/` folder): copy them with
   `sudo cp /Volumes/ac2*/bin/ac2 /Volumes/ac2*/bin/ac2d /usr/local/bin/` and run
   `xattr -d com.apple.quarantine /usr/local/bin/ac2 /usr/local/bin/ac2d`.
   `ac2 --version` prints the build.

Start the app as usual (Launchpad, Finder or Dock). It keeps its log, including the audio
engine it hosts, in `~/Library/Logs/ac2/ac2-ui.log` (Console.app lists it under *Log
Reports*); the previous run's is `ac2-ui.log.1`. Each start replaces the older one, so copy
the file before starting ac2 again if a run went wrong. The app shows its version at the
foot of the connect dialog and on **Settings › Connection** (*Version: ac2-ui …*), the same
string `ac2 --version` prints; the log's `ac2d <version> up` line names it too.

**The app alone, or the app and the command line together.** By default the app runs its
audio engine inside itself (*This computer's audio* in the connect dialog), and the
command line cannot reach that one. The app-only tests use it as is. A test that uses
`ac2` against the app's running session says so, and needs the per-user daemon instead:

1. Quit the app. In Terminal: `ac2 daemon start` (it starts the `ac2d` copied beside `ac2`
   in step 5).
2. Start the app: it finds the daemon and connects to it without the connect dialog (the
   top bar names `ac2d …`). The app and `ac2` now share one session: `ac2 session status`
   and `ac2 devices` read it.
3. Afterwards quit the app, then `ac2 daemon stop`. The next start of the app is the
   built-in engine again. The daemon's log is `~/Library/Logs/ac2/ac2d.log`.

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
- Shortcuts use the Mac keys: ⌘ where ac2 on Linux uses Ctrl, ⌥ (Option) for Alt.
- ac2 never plays anything without a typed level: **L** types it, **Space** arms,
  **Enter** plays, **Shift+Esc** stops from anywhere. Start at **−40 dBFS** and stay at or
  below **−20 dBFS** unless you know your gain structure.
- Note your interface model, its driver or firmware version, the macOS version, and
  whether the Mac is Apple silicon or Intel. These go in the report.

## 3. Test 1: the interface opens and its inputs meter

Pass: the interface is listed, its inputs show live levels, and a session opens without
errors.

1. Start ac2. In the connect dialog choose **This computer's audio (Core Audio)**. Settings
   opens on its **Audio & I/O** page, on the input device.
2. **Input** device (**←/→**): pick your interface. Note what it shows (*N in / M out ·
   rate · buffer*), and whether its inputs carry the interface's own channel names or are
   numbered (*Input 3*).
3. Go down (**↓**) to the input rows. Tap the mic or play something into an
   input: its row's meter must move. Check every input you have.
4. Roles: **↑/↓** to a row, then **R** on input 1 (the loopback), **M** on input 2
   (cable 2; **N** names it, e.g. "cable"), **S** on output 1, and make sure output 2 is in
   the session too (**Space** on its row). Optional: **D** (*Detect loopback…*) at −40 dBFS
   should mark input 1 as the Reference by itself.
5. **Enter** opens the session. Accept the offered transfer measurement with **Enter**.
6. Check the top bar: device, rate and buffer are shown (`<device> · 48.0 kHz · 1024
   frames`); hovering over it shows the stated latency. The **Inputs** list on the left
   keeps a meter per input.

Also worth a try: close the session (**⌘K → Close audio session**), reopen it at
96 kHz if the interface supports it (type the rate on the rate row), and a short buffer
such as 256 frames (the buffer row).

## 4. Test 2: transfer function over a cable, then the delay finder

Pass: an electrical loop reads flat (0 dB, 0°, coherence 1) and the delay finder reports
the interface's own latency between the two inputs, about 0 ms.

1. Make the stimulus play on both outputs: **⌘P**, **⌥1** (Settings › Audio & I/O),
   **S** on output 2 as well, **Esc**. Output 1 feeds the reference over cable 1,
   output 2 the "measurement" over cable 2, so the measurement is a wire and the answer is
   known.
2. **L**, type `-40`, **Enter**; **Space** arms, **Enter** plays pink noise. The transfer
   pane should show magnitude near 0 dB, phase near 0° and coherence near 1 across
   20 Hz–20 kHz.
3. Press **X** (find delay). Expect an arrival close to 0 ms with a PSR figure, not
   *NO ESTIMATE* or *AMBIGUOUS*.
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
4. **G** steps through the pane's views: the impulse response (one sharp peak) and the room
   table (mostly `—` over a cable: there is no decay). Note anything odd, and whether the
   run finished without a dropout warning.

## 5. Test 3: spectrum, RTA and the SPL meter

Pass: each view shows live data that moves with the signal, and the SPL meter reads
plausible levels.

1. **⌘K → New spectrum…** on input 1, then **New RTA…** on input 1. With pink noise
   playing at −40 dBFS over the loopback, the RTA bands should sit roughly level with each
   other (pink noise has equal energy per octave) and the spectrum should slope down about
   3 dB per octave.
2. **⌘K → New SPL meter…** on input 2. Uncalibrated it reads dBFS: with pink noise at
   −40 dBFS on cable 2 it should read close to −40 with Z weighting (**Z** in the SPL pane
   cycles A / C / Z; A reads lower on pink noise) and follow the level when you change it
   with **↑/↓**. A mic on a spare input works too.
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
pass or fail with a named reason per failure. Use the command-line tools from section 1,
step 5, in Terminal. The first run asks for microphone access for Terminal: allow it (a
denial must end in a clean `FAIL` naming the microphone, not a hang; worth noting). **Quit
the ac2 app first** (and `ac2 daemon stop` if you started the daemon) so the device is free.

1. **Silent** (no cable needed; the outputs open but play only zeros):

   ```
   ac2 selftest duplex --backend coreaudio --device "<interface>" --duration 60s
   ```

   `<interface>` is your interface as the app lists it, e.g. `--device "Babyface Pro"`. A
   wrong name prints the devices it knows. Without `--device` it tests the system default
   input. It reports device, channel counts, rate and buffer, the driver's stated latency,
   block sizes, xruns and gaps, callback timing, both clocks against the computer's clock
   and the peak of every input,
   and ends in `result PASS` or one `FAIL` line per reason. `--rate 48khz` and
   `--buffer 256samples` choose rate and buffer; try the smallest buffer you would use.

2. Same again while loading the computer (a browser benchmark, a big build): any dropout
   must show up as a `FAIL` line, not pass silently.

3. **With the loopback cable** (cable 1, output 1 → input 1; optional, plays pink noise on
   output 1 only, at the level you type; nothing but the cable on that output):

   ```
   ac2 selftest duplex --backend coreaudio --device "<interface>" --duration 30s \
       --emit -40dbfs --loopback-out 1 --loopback-in 1
   ```

   It refuses levels above −20 dBFS, fades in and out, and Ctrl-C fades out early. It adds
   the `loopback` line (the output→input offset in samples, whether it held still) and the
   drift between output and input clocks. Run it about five times and note each offset:
   whether it changes between runs is the point.

An aggregate device of two interfaces (made in Audio MIDI Setup) is worth one silent run
too. Paste each report into what you send back (`--json` gives the same as JSON).

## 8. Test 6: the Core Audio backend on a real interface

These answer what the CI loop on a virtual device cannot: a virtual device reports no
latency of its own, cannot be unplugged, has no clock to change and has one stream.

1. **Stated against measured latency.** With the loopback cable (output 1 → input 1), run
   the loopback self-test of Test 5, step 3, with `--rate 48khz --buffer 256samples` added,
   **ten times**. Each report has the driver's statement on its `latency` row
   (`stated N frames in / M out / T total = … ms`) and the measured `loopback` offset in
   samples; note T once and each offset. Pass: the offset is the same in all ten runs, and
   equals T plus a small remainder (the interface's converters; a few tens of samples).
   Note the remainder; a negative one (measured below stated) is worth a line of its own.
   Repeat at a second buffer size (1024 frames): the stated latency changes with the
   buffer, and the remainder should not.
   Then check that a session states the same, with the daemon (section 1, *the app and
   the command line together*): `ac2 daemon start`, start the app, open a session at 48 kHz
   with a 256-frame buffer (Test 1), and run `ac2 session status`: its `stated latency: …`
   line (the same as the top bar's hover) should match the self-test's. Quit the app and
   `ac2 daemon stop` before the next self-test.
2. **Rate change while running.** Open a session with the transfer measurement running
   (Test 2). In Settings › Audio & I/O, on the device rows, **P** brings up **Audio MIDI
   Setup**. There, change the interface's format to another rate (e.g. 44.1 kHz). Pass:
   within a few seconds ac2 reopens by itself at the new rate, and the top bar says so in
   the warning colour (`… · 44.1 kHz · … frames · driver changed from 48.0 kHz`; the hover
   has the details). Change it back afterwards.
3. **Unplug and replug.** With a session open, unplug the interface, wait 10 s, plug it
   back. Pass: the top bar says the audio stopped and that ac2 is reopening, then the
   session comes back by itself within 30 s of the replug. Do it **twice**, then run
   `grep 'audio device probe' ~/Library/Logs/ac2/ac2-ui.log` and paste the lines: each
   `Present { generation: … }` is the interface's object id after a replug, and whether it
   changes between the two replugs is the open question.
4. **Several input streams** (only if the interface has more inputs than its analog ones,
   e.g. ADAT or S/PDIF inputs beside 8 analog: these usually arrive as a second stream).
   Open a session with only input 1 and an input from the second group (e.g. input 9, the
   first ADAT input) in it, and feed each something you can tell apart (the loopback on
   input 1, a tap or a source on the other). Pass: each input's meter moves only with its
   own signal, and the inputs left out of the session cost nothing visible. Note ac2's CPU %
   in Activity Monitor with only inputs 1–2 in the session, then with every input in it.
   With the command line instead: `ac2 selftest duplex --backend coreaudio --device
   "<interface>" --in 1,9 --duration 30s` (silent) against the same without `--in`.

## 9. Optional: record and replay

Checks the file writes and the replay path on macOS.

1. With the session open and pink noise playing, **⌘K**, type `rec`, **Enter** on
   *Record: raw audio of every input on / off*. The top bar shows `REC 0:12 · … MB`. After
   a minute, the same command again stops it; the top bar names the recording
   (`recorded rec-… · 1:00 · … MB`).
2. **Shift+Esc** stops the stimulus. **⌘K → Replay a recording as the session (name or
   path)…**, type that name, **Enter**. The measurements should show the same curves as
   live. Note what the app shows when the recording ends.
3. The files are in `~/Library/Application Support/ac2/recordings/`. Note their size.

## 10. What to send back

A short note per test is enough; failures with a screenshot and the log are the most
useful.

| Item | Your result |
| --- | --- |
| Mac model, Apple silicon or Intel | |
| macOS version | |
| Interface model | |
| Interface driver or firmware version (or "class compliant") | |
| ac2 version (connect dialog foot or Settings › Connection, or `ac2 --version`) | |
| Install: right-click → Open or Open Anyway needed? Microphone prompt shown, and when? | |
| Buffer sizes tried (app and self-test), and any that failed | |
| Test 1: interface listed, channel names or numbers, input meters move, session opens (rate / buffer shown) | |
| Test 2: cables read 0 dB / 0° / coherence 1; delay finder result (ms, PSR) | |
| Test 2 sweep: flat response, distortion level, arrival; finished cleanly? | |
| Test 3: spectrum, RTA and SPL meter live; calibration if tried | |
| Test 4: 1 h run clean? CPU %, memory and battery at start and end | |
| Test 5: self-test reports (silent, under load, loopback offsets if run) | |
| Test 6.1: stated latency (in / out / total frames) at 48 kHz, 256 and 1024 frames; the ten loopback offsets; remainder | |
| Test 6.2: rate change in Audio MIDI Setup: reopened by itself at the new rate, note shown? | |
| Test 6.3: unplug / replug ×2: reopened within 30 s? the `audio device probe` lines | |
| Test 6.4: several input streams: right signal on each input? CPU % with 2 inputs and with all | |
| Record and replay (optional): file written, replay matches live? | |
| Do the input meters and the SPL number move smoothly, or noticeably steppy? | |
| ac2 CPU % in Activity Monitor with a session open and nothing playing | |
| Any banner, freeze, crash or odd behaviour | |

Attach `~/Library/Logs/ac2/ac2-ui.log` (and `ac2-ui.log.1` if the run in question was the
one before) and screenshots of anything that looked wrong.
