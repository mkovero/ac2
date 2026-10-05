# ac2 on macOS — testing with an audio interface

This guide takes a tester from a fresh install to a real measurement on macOS with an audio
interface, in about an hour of hands-on time plus a one-hour unattended run. It covers what
is not verified on macOS yet: Core Audio capture and playback, timing, and long-run
stability. Install and first start (microphone prompt) were confirmed on 2026-10-05.

## 1. Get and install the build

The build is a universal disk image (Apple silicon and Intel, macOS 11 or newer), unsigned
for now.

1. The disk image `ac2-0.0.0-dev.8-macos-universal.dmg` (main f039243, built
   2026-10-05) is in this directory, with `SHA256SUMS` beside it (check with
   `shasum -a 256 -c SHA256SUMS --ignore-missing`). The `.zip` beside it holds the same
   programs unpacked, for use without the disk image.
2. Open the .dmg and drag **ac2** to Applications. If a previous ac2 is there, replace it.
3. First start: **right-click ac2.app in Applications → Open**, then **Open** in the
   dialog. macOS remembers the answer for this one app; there is no need to allow unsigned
   apps in general. On macOS 15, use **System Settings → Privacy & Security → Open Anyway**
   after a first refused start.
4. Allow microphone access when asked. ac2 reads your interface's inputs and nothing else.
5. Optional, for the command-line tools: copy them from the disk image with
   `sudo cp /Volumes/ac2*/bin/ac2 /Volumes/ac2*/bin/ac2d /usr/local/bin/` and run
   `xattr -d com.apple.quarantine /usr/local/bin/ac2 /usr/local/bin/ac2d`.

For the tests below, start the app from Terminal so its log is kept:

```sh
RUST_LOG=info /Applications/ac2.app/Contents/MacOS/ac2-ui 2> ~/ac2-test.log
```

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

1. Start ac2 (Terminal command above). In the connect dialog choose **This computer's
   audio**. The session dialog opens.
2. **Device** (←/→): pick your interface. Note what it shows (*N in / M out · rate ·
   buffer*).
3. Tap the mic or play something into an input: its row's meter must move. Check every
   input you have.
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

1. Make the stimulus play on both outputs: **Ctrl+K → Stimulus: type output channels…**,
   type `1, 2`, **Enter**. Output 1 feeds the reference over cable 1, output 2 the
   "measurement" over cable 2, so the measurement is a wire and the answer is known.
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
3. Optional, if a calibrator is at hand: calibrate the mic input (**Ctrl+K → Input
   setup…**, 94 or 114 dB) and check the meter then reads dB SPL.
4. Maximise the SPL pane (**W**) and switch to the Leq view (**G**). The 1/5/10/30/60 min
   windows should start filling.

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
   `grep -iE "discontinu|overflow|underrun|burst|xrun" ~/ac2-test.log`.
6. Minimise ac2 for a few minutes during the run, then bring it back. It should come back
   at once, not freeze while catching up.

## 7. What to send back

A short note per test is enough; failures with a screenshot and the log are the most
useful.

| Item | Your result |
| --- | --- |
| Mac model, Apple silicon or Intel, macOS version | |
| Interface model, driver or firmware version | |
| ac2 version (top bar, *build …*) | |
| Test 1: interface listed, input meters move, session opens (rate / buffer shown) | |
| Test 2: cables read 0 dB / 0° / coherence 1; delay finder result (ms, confidence) | |
| Test 3: spectrum, RTA and SPL meter live; calibration if tried | |
| Test 4: 1 h run clean? CPU %, memory and battery at start and end | |
| Any banner, freeze, crash or odd behaviour | |

Attach `~/ac2-test.log` and screenshots of anything that looked wrong.
