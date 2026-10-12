# Core Audio on hosted macOS CI runners (BlackHole)

Hosted macOS runners can run a real Core Audio duplex loop through the BlackHole virtual
loopback driver, the macOS counterpart of the Windows FlexASIO-on-VB-CABLE pass, on both
arm64 and Intel, with no permission work on the current images. Measured in a spike
(2026-10-12, branch `spike/macos-blackhole`, runs
[38193893277](https://github.com/mkovero/ac2/actions/runs/38193893277) to
[38197232241](https://github.com/mkovero/ac2/actions/runs/38197232241)); the spike's probes
(`hal-timestamps`, a Core Audio HAL IOProc in Swift; `cpal-ts`, a standalone cpal 0.18
timestamp program; `tcc.sh`) stay on that branch under `tools/experiments/macos-blackhole/`. CI uses the recipe in *In CI* below.

The findings below were made through cpal. Since `coreaudio.md` stage (b) the BlackHole
checks run on the macOS default, the direct HAL backend `coreaudio`; its results on this
runner are in `coreaudio.md` §12, *Settled on CI*.

Runner images seen: `macos-latest` = image `macos-26-arm64` 20260907, macOS 26.6.2 (25G83),
arm64. `macos-15-intel` = image `macos-15` 20260824, macOS 15.7.9 (24G830), x86_64.
`macos-13` is retired (December 2025); `macos-15-intel` is the last Intel label (until the
macOS 15 image retires, fall 2027).

## Audio safety

The runners have no audio hardware: before the install the only devices are Apple's
"Null Audio Device" (Intel) and "Apple Virtual Sound Device" + "Null Audio Device" (arm64),
all `Transport: Virtual`. ac2 itself never emits: no `--emit`, no generator; the outputs of
every `selftest duplex` and session play silence (`stop: never_emitted`), as in the Windows
job.

A capture check needs non-zero samples, so the stimulus comes from outside ac2, into
BlackHole only: sox (`-40 dBFS` 997 Hz sine; for the delay finder pink noise at `-30 dB`
gain on both channels, the second delayed 480 samples). The operator allowed this for CI
(2026-10-12) on these terms: sox refuses to play unless `GITHUB_ACTIONS=true` and
`RUNNER_ENVIRONMENT=github-hosted`, and only to a device named `BlackHole…`. None of this
belongs on a real machine.

## Q1. Install and visibility

```
brew install --cask blackhole-2ch          # BlackHole 0.7.1, pkg run with sudo (passwordless on runners)
sudo launchctl kickstart -kp system/com.apple.audio.coreaudiod || sudo killall coreaudiod
```

Works on both labels. The cask prints `You must reboot for the installation of
blackhole-2ch to take effect` and `installer: The install requires restarting now.`; no
reboot is needed. Before the coreaudiod restart `system_profiler SPAudioDataType` does not
list BlackHole; right after it (3 s settle) it does:

```
BlackHole 2ch:
  Input Channels: 2
  Manufacturer: Existential Audio Inc.
  Output Channels: 2
  Current SampleRate: 48000
  Transport: Virtual
```

It does not become the default device (Null Audio Device / Apple Virtual Sound Device stay
default), so ac2 must address it explicitly. Cost: the cask plus `sox` took well under a
minute; the debug build of `ac2-cli` + `ac2d` from cold took 3 m 48 s on arm64 and
11 m 21 s on Intel (no cache).

## Q2. Microphone permission (TCC)

**No grant is needed on the current images.** SIP is disabled (`csrutil status`: `System
Integrity Protection status: disabled.`) and the image already carries, in both the user and
system `TCC.db`:

```
kTCCServiceMicrophone | /opt/hca/hosted-compute-agent           | client_type 1 | auth_value 2 | auth_reason 4
kTCCServiceMicrophone | /usr/local/opt/runner/provisioner/provisioner | 1 | 2 | 4
kTCCServiceMicrophone | /opt/hca/start_hca.sh                   | 1 | 2 | 0
kTCCServiceMicrophone | /usr/local/opt/runner/runprovisioner.sh  | 1 | 2 | 0
```

TCC attributes a CLI's capture to the responsible process at the top of its chain. On both
labels the chain is `bash → Runner.Worker → Runner.Listener → /opt/hca/hosted-compute-agent
→ launchd`. With every microphone row deleted except `/opt/hca/hosted-compute-agent`'s
(run 38197232241, step 4), capture still gets samples on both labels; that row alone is the
working grant. From the first run on, with no TCC change at all:

- `AVCaptureDevice.authorizationStatus(for: .audio)` = 3 (authorized) in the Swift probe;
- sox records BlackHole: `Maximum amplitude 0.010000`, `RMS amplitude 0.007071` (the
  -40 dBFS sine exactly);
- `ac2 selftest duplex`: `peak_dbfs [-40.000008, -40.000008]`.

**Without a grant, capture blocks instead of returning zeros.** With every
`kTCCServiceMicrophone` row deleted and `tccd` restarted, `authorizationStatus` reads 0
(`notDetermined`), and every Core Audio capture client hangs: sox records nothing, the HAL
probe never returns from `AudioDeviceStart`, and `ac2 selftest duplex` prints `ac2: running
8 s, outputs silent (Ctrl-C stops early)` and never finishes (killed by a 40 s alarm, exit
142). macOS is waiting for a consent prompt nobody can answer. In run 38194653217 the
unbounded version of this step ran into its 5 min timeout (`The action 'Capture with
microphone revoked, then granted to the runner agent only' has timed out after 5
minutes.`); in run 38195766309, with each command bounded, re-inserting the agent row
right after the hung attempt did not unblock the next capture either (the pending request
seems to wedge Core Audio clients until the runner ends), so the denied case now runs last. Consequences:

- A CI job must bound every capture command with a timeout; a missing grant shows up as a
  hang, not as silence.
- If a future image drops the row, this restores it (runner only, SIP is off there):

  ```
  for db in "$HOME/Library/Application Support/com.apple.TCC/TCC.db" \
            "/Library/Application Support/com.apple.TCC/TCC.db"; do
    sudo sqlite3 "$db" "INSERT OR REPLACE INTO access
      (service, client, client_type, auth_value, auth_reason, auth_version, flags, last_modified)
      VALUES ('kTCCServiceMicrophone', '/opt/hca/hosted-compute-agent', 1, 2, 4, 1, 0, $(date +%s));"
  done
  sudo killall tccd; killall tccd
  ```

  The insert itself was exercised (run 38195766309); whether it alone recovers a fresh
  runner was not, because no image lacked the row. It must run before any capture.
- Real Macs: a capture with access denied returns zeros per Apple's documentation; this
  spike saw only the undetermined case. ac2's `access_denied_hint` in `cpal_host.rs` is
  Windows-only today.

## Q3. ac2 on BlackHole through cpal

**Device list.** `ac2 devices` (daemon `--backend cpal`) lists it on both labels:

```
{"name":"BlackHole 2ch","id":"coreaudio:BlackHole2ch_UID","in":2,"out":2}
```

The id is the device UID, stable across runs and both architectures; select by id
(`--device coreaudio:BlackHole2ch_UID`) or by name (`--device "BlackHole 2ch"`, which
`selftest duplex` accepts). One device object serves both directions, so ac2 reports
`clock: same_device_separate_callbacks`, `index: estimated`, f32 both ways, 48 kHz.

**`ac2 selftest duplex --backend cpal --device "BlackHole 2ch" --in 1-2 --outputs 2
--duration 8s`** (silent outputs, sox sine on the cable):

| label | buffer | pass | peak dBFS | xrun blocks / index gaps | in clock ppm | out clock ppm | input lag | playback lead |
|---|---|---|---|---|---|---|---|---|
| arm64 | 1024 (default) | yes | -40.00 / -40.00 | 0 / 0 | 2.2e-7 | 2.2e-7 | 21.333 ms | 21.333 ms |
| arm64 | 512 | run 2: no (`xruns 2`, `index_gaps 1, 1536 frames`); runs 3, 4: yes | -40.00 | 1 / 1; 0 / 0 | ~1e-6 | ~1e-6 | 10.667 ms | 10.667 ms |
| arm64 | 2048 | yes | -40.00 | 0 / 0 | 8.9e-7 | -4.4e-7 | 42.667 ms | 42.667 ms |
| Intel | 1024 (default) | yes (all 4 runs) | -40.00 | 0 / 0 | -1.7e-5 | -2.2e-5 | 21.333 ms | 21.333 ms |
| Intel | 512 | no, all 3 runs (`xruns 4`, `index_gaps 2, 4096 frames`; `xruns 2`, `index_gaps 1, 8192 frames`; `xruns 2`, `index_gaps 1, 1536 frames`) | -40.00 | 1–2 / 1–2 | -2e-5 | -3e-5 | 10.667 ms | 10.667 ms |
| Intel | 2048 | yes | -40.00 | 0 / 0 | -1.7e-5 | -2.8e-5 | 42.667 ms | 42.667 ms |

**30 s daemon session** (`session open --backend cpal --device coreaudio:BlackHole2ch_UID
--out-device coreaudio:BlackHole2ch_UID --in 1-2 --outputs 2`, `rec start`, tf `--ref 1
--meas 2` running, `delay find` after 15 s):

| label | buffer | recorded frames | discontinuities | delay finder |
|---|---|---|---|---|
| arm64 | 1024 | 1 378 304 | 0 (all 4 runs) | accepted, 480.0 samples (10.000 ms), uncertainty 0.1, PSR 73.2 dB |
| arm64 | 2048 | 1 372 160 | 0 | accepted, 480.0 samples, PSR 73.4 dB |
| Intel | 1024 | 1 372 160 | 0 / 4 / 8 / 3 across runs 1–4 | run 3: `no_estimate` (`observation_too_short`); run 4: accepted, 480.0 samples, PSR 73.0 dB |
| Intel | 2048 | 1 390 592 | 0 | accepted, 480.0 samples, PSR 73.5 dB |

- **Delay.** The finder recovers sox's 480-sample offset between the two inputs exactly,
  every time it had live audio. This is an input-to-input delay, not ac2's output→input
  loopback timing: that needs `--emit`, which no automated run passes. Recorded levels
  match the source on both channels (pink noise, RMS 0.0070, peak 0.031).
- **Discontinuities.** Every discontinuity on Intel at 1024 frames is flagged
  `XRUN|DISCONTINUITY|GAP_ESTIMATED` (`ac2d::fanout: capture discontinuity at sample 11324`),
  i.e. Core Audio reported an overload and the timestamp jumped. They come with the
  tf running on a debug build on the slower Intel VM, and with 512-frame buffers on either
  label. cpal-ts, which does almost nothing per callback, also got `input error: A buffer
  underrun or overrun occurred.` once in some runs (a single late cycle). ac2 at 2048 frames lost nothing in
  any run. Hosted runners are a shared VM, not a real-time machine.
- **Clock.** Measured against the host clock the device runs at 48 000 Hz within 1e-4 ppm:
  BlackHole is clocked from the host clock, so this checks the arithmetic, not a crystal.
- **`TimestampClock` looks sane.** Capture interval spread is ~2 ns (the timestamps are
  computed from Core Audio's ideal `mHostTime`, not wall time), so a gap appears as an exact
  multiple of the buffer: 1536 = 3 × 512, 4096 = 8 × 512, 8192 = 16 × 512 (whole cycles), and
  nothing is flagged otherwise. `input lag` and
  `playback lead` are exactly one buffer each, and constant; see Q4 for what that means.

## Q4. Core Audio's own timestamps beside cpal's

`hal-timestamps` (HAL IOProc, same device) on BlackHole, both labels alike:

- `kAudioDevicePropertyLatency` 0 and `kAudioDevicePropertySafetyOffset` 0 on input and
  output; buffer 512 by default, settable to 1024 / 2048.
- `inInputTime.mSampleTime` advances exactly by the buffer every callback (0 gaps);
  `mRateScalar` 1.000000000 (arm64), 0.999999999 (Intel); flags 0x7 (sample time, host
  time, rate scalar valid).
- Per cycle: `inNow − inInputTime` = 1 buffer (10.7 ms at 512, 21.3 ms at 1024);
  `inOutputTime − inInputTime` = 2 buffers in both sample time and host time, on one
  timeline (sample time ↔ host time is the same map on both sides).
- Click loopback: a click written at output sample time T is read at input sample time T,
  every click, both buffer sizes, both labels (`loopback 0 samples`). BlackHole is a
  zero-latency loop in its own sample-time domain.
- The `now` host time of the callback itself jitters (arm64 cycle-to-cycle 9.2–12.7 ms at a
  10.67 ms buffer); the `mHostTime` values do not.

`cpal-ts` (cpal 0.18.2 on the same device, `BufferSize::Fixed`), measured against
`mach_absolute_time()` read in each callback:

| | 1024 frames | 2048 frames |
|---|---|---|
| input: `now − callback` | +22.6 ms (arm64), +21.9 ms (Intel) | +44.6 ms, +43.2 ms |
| input: `capture − callback` | −21.333 ms, constant | −42.667 ms |
| output: `now − callback` | −20.1 ms (arm64), −20.8 ms (Intel) | −40.8 ms, −42.2 ms |
| output: `playback − callback` | +21.333 ms | +42.667 ms |
| output `callback` − input `callback`, same cycle | 42.667 ms | 85.333 ms |
| click: cpal `capture − playback` | **−42.667 ms = −2048 frames** | **−85.333 ms = −4096 frames** |

cpal's `callback` instant is not the time the callback ran: for input it is the AUHAL
timestamp, one buffer before the callback ran (the capture time of the first frame, as the
HAL's `inInputTime`); for output it is one buffer after (the play time of the first frame,
as `inOutputTime`). cpal then subtracts (input) or adds (output) a further `buffer + device
latency + safety offset` (`cpal-0.18.2/src/host/coreaudio/macos/device.rs`, input around
line 787, output around line 913). So on Core Audio cpal's `capture` is one buffer early and
its `playback` one buffer late, and a loop that is 0 in the device's own time reads as
**−2 buffers** from cpal's timestamps: a negative latency, which cannot happen.

What this means for ac2 today: the offset is constant for a given buffer size, and
`q3-loopback-timing.md` uses only the stability of cpal's offset, so drift and slips are
unaffected. Anything that turns cpal timestamps into an absolute output→input latency on
macOS is off by 2 × buffer (plus the device latency and safety offset twice over on real
hardware, where they are not 0; BlackHole cannot show whether the AUHAL timestamp already
includes them). A direct Core Audio backend would get exact sample-time indexes on both
sides (`mSampleTime`, one timeline for both directions on one device) and the device's
own latency properties, which is the same position ASIO and JACK give ac2 now. Not
measured here: aggregate devices, two-device setups, real hardware's latency properties.

## In CI (`ci.yml`, all_os runs)

The `test` job on `macos-latest` (arm64 only: Intel adds ~8 min of build for the same
result and its label ends in 2027), after the tests, with the debug binaries they built:

1. `brew install --cask blackhole-2ch`, `brew install sox`, restart coreaudiod.
2. `tools/release/macos-blackhole.sh target/debug virtual-audio-logs`, the twin of
   `tools/release/windows-asio.ps1`:
   - waits (≤ 30 s) until `system_profiler SPAudioDataType` lists `BlackHole 2ch`;
   - asserts the `kTCCServiceMicrophone` row for `/opt/hca/hosted-compute-agent` (user or
     system `TCC.db`) instead of writing it, and fails naming the fallback in Q2 if missing;
   - `ac2d --backend cpal`: `ac2 devices` lists BlackHole with inputs and outputs;
   - `selftest duplex --buffer 2048samples --duration 20s` with the sox sine: pass, and the
     captured peak −40 ± 1 dBFS on both inputs;
   - a 30 s daemon session at `--buffer 2048samples` with a recording and the tf running on
     the sox pink noise: frames ≥ 90 % of expected, 0 discontinuities, and `delay find`
     accepted at 480 ± 1 samples.

   Every ac2 call runs under a deadline, since a missing grant hangs instead of failing.
   2048 frames because 1024 lost cycles on Intel and 512 on both, and a CI check must not
   flake on VM scheduling.
3. Logs (every JSON reply, daemon stderr, `system_profiler`) are uploaded as
   `virtual-audio-logs-macOS`.

The delay check is input-to-input (sox delays one channel), not ac2's output→input loopback
timing: that needs `--emit`, which no automated run passes.
