# Spike: audio duplex (phase 0)

> **Concluded; spike code removed.** Findings and the decisions they led to. The backends
> live in `crates/ac2-audio`; Linux is JACK only (decision L1 in `open-questions.md`), so
> the cpal-ALSA results below no longer apply to a shipped path; Windows is ASIO only
> (decision W1, `asio.md`), so neither do the cpal-WASAPI ones. The spike's run mode lives
> on as `ac2 selftest duplex` (`crates/ac2-cli/src/selftest`); §8 is its hardware procedure.

**Question.** Can one small trait give RT-safe, multichannel, sample-indexed duplex capture
on all three OSes, using cpal (CoreAudio / WASAPI / ALSA) and JACK (the `jack` crate)?

**Answer.** Yes. Sample indexing is **exact on JACK** and **estimated on cpal**: cpal gives
no hardware frame position on any host. On cpal, input and output are always **two separate
streams**, so the generator→loopback offset is fixed only when both run on one device clock,
and even then it can change on every stream start. Decision 3a (continuous loopback
correlation) exists to catch exactly that. JACK was proven against a dummy server, ALSA
through cpal on the `null` PCM; macOS and Windows were not run on hardware (§8).

Spike versions: cpal 0.18.2, jack 0.13.5, rtrb 0.4.

## 1. Verdict per OS / backend

| Backend | Duplex model | Sample index | Gap / xrun signal | Static latency | Status |
|---|---|---|---|---|---|
| JACK (Linux) | one process callback, one clock | **exact** (`last_frame_time`, u32 wrap extended) | exact frame-counter jump + async xrun notification | port latency ranges (frames) | **works**: dummy server, 10 s, 0 gaps, 0 drops |
| cpal ALSA (Linux) | 2 PCMs, 2 threads, no `snd_pcm_link` | running count + timestamp gap estimate | `ErrorKind::Xrun` on EPIPE (cpal recovers itself) + htstamp jump | per callback: `capture = htstamp − delay` | **works** on `null` PCM (plumbing only; no real clock) |
| cpal CoreAudio (macOS) | 2 AUHAL units, 2 IOProcs; one device can be both | running count + timestamp gap estimate | `kAudioDeviceProcessorOverload` → `Xrun` | per callback: `capture = mHostTime − (buffer + latency + safety offset)` | **unverified** (compiles in principle; needs HW) |
| cpal WASAPI (Windows) | 2 `IAudioClient`s, 2 event threads, **shared mode only** | running count + timestamp gap estimate | capture `DATA_DISCONTINUITY` → `Xrun`; **render xruns not reported** | per callback: QPC from `GetBuffer`; output adds `GetStreamLatency` | **unverified**; same-clock cannot be proven via the API |
| fake | single callback thread, scripted loopback & faults | exact | scripted | — | **works**; basis of the hardware-free tests |

ASIO (not tested, feature-gated, phase 7) is the only cpal host where both directions share a
single driver `bufferSwitch` callback.

## 2. What carried into `ac2-audio`

The spike's pieces became the shared, backend-agnostic core of `ac2-audio`:
- `block.rs`: `BlockHeader {start_sample, frames, channels, flags, callback_ns, capture_ns}`
  over two SPSC `rtrb` rings (interleaved samples, then headers). The sample commit is
  sequenced before the header's release store, so a popped header always has its samples.
  A block is written whole or dropped whole; a drop sets `OVERFLOW|DISCONTINUITY` on the
  next block that gets through.
- `clock.rs`: `FrameCounterClock` (JACK: extends the u32 counter across its ~24.8 h wrap at
  48 kHz; any deviation from `prev + n` is an exact `DISCONTINUITY`) and `TimestampClock`
  (cpal: running count; a capture timestamp more than half a block past prediction adds the
  estimated missing frames and sets `DISCONTINUITY|GAP_ESTIMATED`).
- `events.rs`: `EventLatch` turns asynchronous xrun/config notifications (atomics) into flags
  on the next block.
- `output.rs`: silence by default, fades, and a global level limit (`MaxLevel`); stop fades
  out before the stream drops.

## 3. Measured (Linux, no sound hardware, no RT privileges)

| Run | Result |
|---|---|
| JACK dummy (`jackd -d dummy -r 48000 -p 256 -C 4 -P 2`), 10 s, 4 in / 2 out, release | 1876 blocks of 256 frames, **0 index gaps, 0 regressions, 0 drops, 0 xruns**. Wake interval stddev 181 µs, max deviation 3.5 ms (not RT-scheduled); DLL cycle-start stddev 0.86 µs. Port latency capture 256 / playback 512 frames. |
| Same, debug build, 5 s | 5 xruns, 3 exact index gaps (1024 frames), all flagged (non-RT scheduling). |
| cpal ALSA `null`, 3 s | No clock, callbacks spin at ~1.6 µs: a transport stress test. 620 k blocks delivered, 3.9 M dropped; **every one of the 1350 index gaps carried `OVERFLOW`**, 0 regressions, channels never misaligned. |

Two findings shape the trait:

1. **A late JACK cycle need not skip frames.** A callback woke 8.8 ms after the previous one
   (nominal 5.3 ms) with `last_frame_time` contiguous. On real hardware an ALSA driver xrun
   loses ADC samples while JACK's frame time counts *cycles*, so the index can look
   contiguous across lost audio. **`XRUN` breaks continuity by itself** (it is in
   `BREAKS_CONTINUITY`).
2. **The xrun notification is asynchronous** (JACK's notification thread), so `XRUN` can land
   one block after the affected block. Consumers treat the block before an `XRUN` as suspect.

## 4. cpal 0.18 per host (from source)

Paths below are relative to `cpal-0.18.2/src`.

**No duplex API on any host.** Only `build_input_stream[_raw]` and
`build_output_stream[_raw]` exist (`traits.rs:261-423`). Input and output are two callbacks.
cpal neither aligns them nor tells you their phase.

**Same device / clock**

- **CoreAudio:** one `Device` covers both directions. The id is the device UID; equality
  compares `AudioDeviceID` (`coreaudio/macos/device.rs:431-466,958`). Same id means same clock.
  - cpal only *detects* aggregate devices (`InterfaceType::Aggregate`). It creates a private
    aggregate only for loopback capture of output-only devices.
  - Combining two interfaces into one clock is the user's job, in Audio MIDI Setup.
- **ALSA:** one PCM name for both directions (`hw:CARD=X,DEV=0`), so you can parse `CARD=`.
  `snd_pcm_link` is not used, so the two PCMs start independently.
- **WASAPI:** capture and render endpoints have **different ids** even on the same card, and
  there is no ContainerId. The only grouping hint is the friendly interface name in
  `description().driver()`. `ClockRelation` must stay `Unknown` here unless the user asserts
  otherwise or drift detection shows zero drift.

**Latency and timing information**

`InputCallbackInfo`/`OutputCallbackInfo` carry only `{callback, capture}` and
`{callback, playback}` `StreamInstant`s (`timestamp.rs:45-75`).

- **CoreAudio:**
  - `callback` is `AudioTimeStamp.mHostTime`.
  - `capture`/`playback` are `callback ∓ (BufferFrameSize + kAudioDevicePropertyLatency +
    SafetyOffset)`, queried once at build time.
  - `mSampleTime`, the real sample position, is **not exposed**.
- **WASAPI:**
  - Input `capture` is the QPC position from `GetBuffer`. That call also returns a 64-bit
    device frame position, which cpal **does not expose**.
  - Output `playback = callback + (written − device position) + GetStreamLatency()`.
- **ALSA:**
  - `htstamp`, from `snd_pcm_status` with audio_htstamp config (`MONOTONIC_RAW`);
    `capture = callback − delay`.
  - The ALSA PCM has an exact frame counter, which is not exposed.
- **JACK** (via cpal; not used here): worst-case port latency.

**Xruns**

- ALSA reports EPIPE as `Xrun`, then recovers by itself (`prepare` + `start`) and keeps
  running.
- CoreAudio reports processor overload.
- WASAPI reports capture discontinuity only.
- All real-time paths deliver errors via `try_emit_error`, which **drops the error if the
  error-callback mutex is contended** (`host/error_emit.rs:24-40`). Timestamp gap detection is
  therefore a necessary backstop, not a nicety.

**Buffer size**

- **ALSA:** `Fixed(n)` sets the period and forces 2 periods. The callback is always exactly
  one period.
- **CoreAudio:** `Fixed` sets `kAudioDevicePropertyBufferFrameSize`, which is
  **device-wide**, so input and output share it.
- **WASAPI:** `Fixed` only sizes the ring. The callback size **varies** (one input callback
  per capture packet; output gets size − padding). `SupportedBufferSize` is min = max =
  device period.
- **JACK:** must equal the server's size.

**Formats**

- cpal does **no conversion**; the typed API panics on a mismatch. The spike opens the
  native format via `_raw` and converts f32/i32/i24/i16 itself.
- ALSA `hw:` usually needs i16 or i32; `plughw:` converts in alsa-lib.
- CoreAudio advertises f32 only, because the AUHAL converts.
- WASAPI shared mode is usually f32.

**Channels**

- ALSA: up to 64.
- CoreAudio: sum of stream channels.
- WASAPI: only the mix format's `nChannels`. A 16-channel interface in shared mode may show
  only its mix-format channels.

**RT safety inside cpal**

- **ALSA:** the hot path does not allocate (poll, `avail_delay`, `status`, `readi`/`writei`).
  - Real-time promotion needs the `realtime` feature (plus `realtime-dbus` on Linux).
  - It is **skipped for `plug`/`default` PCMs**.
- **CoreAudio:** runs on the HAL real-time IO thread. Each output callback calls
  `mach_timebase_info` (commpage, cheap).
- **WASAPI:** makes COM calls per packet. MMCSS needs the `realtime` feature.

The spike's own callbacks do only rtrb writes, atomics and arithmetic.

## 5. Per-OS gaps

- **Linux:**
  - JACK is the clean path.
  - cpal-ALSA works, but the two PCMs are not linked: the start phase between capture and
    playback is arbitrary per open.
  - Pick `hw:` (exact format, needs conversion) or `plughw:` (conversion, but no RT
    promotion).
  - PipeWire systems: either use JACK via pipewire-jack, which gives the single-callback
    model and is preferred, or cpal's `pipewire` feature. Not evaluated here.
- **macOS:**
  - Single device = single clock, but still two IOProcs. No exposed `mSampleTime`, so the
    index is estimated.
  - Two interfaces need a user-made aggregate device with drift correction off/understood.
  - Buffer size is device-wide and can be changed by other apps → watch `CONFIG_CHANGE`.
  - Microphone permission (TCC) prompt on first capture.
- **Windows:**
  - Shared mode only: the engine mixes and resamples, so the stream rate is the endpoint mix
    rate. There is no exclusive mode, so no bit-exact path and higher latency (about 10 ms
    periods).
  - Clock identity between capture and render endpoints is not provable.
  - Render xruns are invisible.
  - Variable callback sizes.
  - ASIO (phase 7, licensing) is the real pro-audio path.

## 6. Trait shape

Implemented in `crates/ac2-audio/src/backend.rs` (`Backend`: `kind`, `enumerate`, `open`,
`probe`; `DeviceCaps`, `DuplexRequest`, `Negotiated`, `ClockRelation`
{`SingleCallback`, `SameDeviceSeparateCallbacks`, `Unknown`}, `IndexExactness`,
`StaticLatency` {`PortRanges`, `PerCallbackTimestamps`, `Unknown`}) and `stream.rs`
(`DuplexStream`, concrete, not a trait object). Design points from the spike:

1. **Keep the trait object-safe and tiny.** The work lives in shared pieces unit-tested with
   the fake backend (block transport, the two clocks, `EventLatch`, output renderer); each
   real backend is a few hundred lines of glue.
2. **`Negotiated` states how exact the index is.** Gap flags add `GAP_ESTIMATED` when the
   size is a guess; jobs reset on any `BREAKS_CONTINUITY` flag regardless.
3. **Generator index ≠ capture index on cpal**: separate counters on separate threads. Only
   JACK, ASIO and fake (one callback) make `out_start_sample == in_start_sample` meaningful.
   `OutputTick` lets a job relate them through timestamps; loopback correlation (3a) is the
   truth.
4. **Ring transport:** two SPSC rings, whole-block-or-nothing, overflow carried as a flag.
   Interleaved; jobs deinterleave on their own thread. Size the header ring from the
   *smallest* plausible block, because WASAPI packets vary.
5. **One stream per session, fan-out after the ring (PLAN §4.3)**; no multiple consumers in
   the callback.
6. **Don't use cpal's JACK host**: it creates a client per stream and loses the
   single-callback property. Use the `jack` crate directly.
7. **Not built (optional, post-1.0):** native CoreAudio/WASAPI backends or a cpal patch
   exposing `mSampleTime` / the WASAPI device position for an exact index on mac/win.

## 7. Risks for Q3 (continuous loopback timing monitor)

- **Offset changes on every stream (re)start on cpal.** Input and output are started by two
  `play()` calls on two threads or units, with no link, so the output→input sample offset is
  a new random value each time. That is fine because there is no start-up probe, but every
  stream start, device change and `CONFIG_CHANGE` must start a new "offset epoch" and
  re-acquire. This lines up with 5b (session epochs).
- **Silent offset jumps.** cpal-ALSA recovers xruns internally (and with only the capture or
  only the playback PCM re-prepared, the relative offset shifts by the lost frames).
  WASAPI never reports render xruns. Error delivery can be dropped (`try_lock`). The
  continuous correlation is the only reliable detector, so it must run often enough to catch
  a jump within about 1 s while stimulus plays. Silence (no stimulus) means jumps are
  detected only when stimulus resumes, which is the decided behaviour (last value + age).
- **Index estimation error on cpal.** A timestamp-estimated gap may be off by a few frames,
  especially where `capture` is computed from `callback` (ALSA delay, WASAPI QPC). Jobs reset
  on gaps anyway. The loopback correlation will see the true offset; the index error only
  affects how long a gap appears to be.
- **Clock domain on Windows.** Capture and render are separate endpoints in shared mode with
  engine SRC. If the user picks two different devices, the offset drifts continuously
  (PLAN §12: about 600 µs over 6–30 s). Drift detection is the correlation slope, and it must
  warn.
- **Output timing records are host estimates.** `playback_ns` on CoreAudio is built from
  static latency properties, so it misses converter, console and network latency. Q3 already
  says it is plausibility only, and the spike confirms that.
- **JACK late cycles look contiguous** (§3). The `XRUN` flag, not the frame counter, must
  trigger an offset re-check.
- **CoreAudio device-wide buffer size** can be changed by another app mid-run. Does cpal
  surface that as an error? Unknown (§8).

## 8. Manual hardware verification: `ac2 selftest duplex`

The spike's run mode lives on as `ac2 selftest duplex`: it opens the production backend in
the `ac2` process (no daemon; stop one that holds the device) and prints a report ending in
`result PASS` or one `FAIL` line per named reason; `--json` gives the same as one document.
Exit status 0 passes, 1 fails. Release binaries are enough; testers paste the text report.

What it judges. Silently (the default; outputs open but play zeros): the stream opens;
channel counts; block sizes; xrun, discontinuity, overflow and config-change flags; index
gaps and regressions; host xrun / error / end notifications; callback and capture interval
spread, input lag, output playback lead; per-input peak; and both clocks against the host
clock (`input rate`, `output rate`, failing beyond ±1000 ppm). With `--emit <dBFS>` (pink
noise, at most −20 dBFS, fades in and out) and `--loopback-out` / `--loopback-in` on a cable,
it also runs the daemon's loopback timing monitor: the output→input offset, locks, jumps,
losses, and the in/out drift (judged after 10 s; a drift warning fails as `ClockDrift`).

Run on one machine per OS, with a real interface. Silent steps need no cable. Emitting steps
need a **physical cable from the loopback output to the loopback input**, monitors off; a
human runs them, never an agent.

**All OSes**

1. `ac2 devices` (daemon running) or the error of a misnamed `--device` lists the
   interfaces. Check that every interface appears, that channel counts match the hardware
   (WASAPI: does a multichannel interface show all its inputs?), and the rate and buffer
   ranges.
2. Silence, 60 s: `ac2 selftest duplex --backend cpal --device <id> --duration 60s`.
   Expect PASS, one block size (Core Audio), the `clock` line `one device, separate
   callbacks` on single-device interfaces on macOS and `separate devices or endpoints` on
   Windows. Record the interval spreads, input lag and playback lead.
3. Multichannel: `--in 1,4,8` on an 8+ input interface, a signal (a mic, an external
   generator) into input 4 only. `input peaks` must show it on 4 only, the others at the
   noise floor.
4. Stress: step 2 while loading the CPU (`stress -c $(nproc)` or a browser benchmark).
   Every xrun must fail the run by name (`xruns`, `sample-index gaps`); compare with the
   OS's own indication (Core Audio overload in Console.app; Windows: none).
5. Hot-unplug the USB interface during a 30 s run. Expect `stream errors` or `stream ended`
   (or a configuration change), no crash or hang, and the command returning.
6. Change the buffer size from another app mid-run (macOS: Audio MIDI Setup or a DAW;
   Windows: the sample-rate setting). Record which failures appear.
7. Restart offset repeatability, the core Q3 risk (emits; loopback cable; monitors off):
   `ac2 selftest duplex --backend cpal --device <id> --duration 20s --emit -40dbfs
   --loopback-out 1 --loopback-in 1`, ten times. Record the `loopback offset` of each run.
   Expected: varies between runs on cpal, constant on JACK. Each run must itself PASS (no
   jumps, no loss, no drift).

**macOS specific**

8. Aggregate device of two interfaces: listed as one device; a silent run must PASS. With
   the loopback cable from one member to the other, step 7 shows the drift with drift
   correction off, and none with it on.
9. Microphone permission: the first run from Terminal must prompt. After denial, expect a
   clean failure (`the stream did not open` or `no capture block arrived`), not a hang.

**Windows specific**

10. Capture and render endpoints of the same interface: `--device <capture id>
    --out-device <render id>`. Confirm the stream rate equals the endpoint's mix rate in
    Sound settings; a mismatched `--rate` must fail to open. With the cable, step 7's drift
    must stay below the warning (one clock behind both endpoints).
11. Block sizes at the default period (expect about 480 frames at 48 kHz) and with
    `--buffer 128samples`.

**Linux real hardware (lower priority; JACK is proven on a dummy server)**

12. `jackd -d alsa -d hw:X -r 48000 -p 256`, then `ac2 selftest duplex --backend jack
    --duration 60s`. Expect PASS with RT privileges. Then force an xrun (`-p 32` under load)
    and confirm it fails as `xruns`.

## 9. Compared to `ac`

`ac`'s cpal backend locked and allocated in the callback, read only channel 0, was fixed at
44.1 kHz and had no routing. Kept from `ac` as ideas: SPSC rings, sample-aligned capture, port
latency ranges as plausibility data. New: a sample index on every block, and gap/overflow
flags in-band.
