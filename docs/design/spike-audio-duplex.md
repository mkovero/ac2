# Spike: audio duplex (phase 0)

> **History.** A record of what the phase 0 spike (`spikes/audio-duplex`) measured, not a
> description of the product. The backends live in `crates/ac2-audio`; Linux there is JACK
> only (decision L1 in `open-questions.md`), so the ALSA-through-cpal results below no longer
> apply to a shipped path.

**Question.** Can one small trait give RT-safe, multichannel, sample-indexed duplex capture
on all three OSes, using cpal (CoreAudio / WASAPI / ALSA) and JACK (the `jack` crate)?

**Short answer.** Yes for the transport and the trait. Sample indexing is **exact on JACK**
and **estimated on cpal**: cpal gives no hardware frame position on any host. On cpal,
input and output are always **two separate streams**, so the generator→loopback offset is
only fixed when both run on one device clock. Even then it can change on every stream
start. That is acceptable under decision 3a: continuous loopback correlation exists to
catch exactly that. JACK was proven locally against a dummy server, and ALSA was proven
through cpal on the `null` PCM. **macOS and Windows compile paths exist but are unverified**:
there is no hardware here, and only the Linux target is installed. The manual steps are in §8.

Code: `spikes/audio-duplex` (throwaway). cpal 0.18.2, jack 0.13.5, rtrb 0.4.

---

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

## 2. What was built

- `block.rs`: `BlockHeader {start_sample: u64, frames, channels, flags, callback_ns,
  capture_ns}` and the transport. The callback writes interleaved f32 samples to one
  `rtrb` ring, then the header to a second ring. The sample commit is sequenced before the
  header's release store, so a popped header always has its samples available. A block is
  written whole or dropped whole. A drop sets `OVERFLOW|DISCONTINUITY` on the next block
  that gets through and bumps the drop counters. The callback never allocates, locks or blocks.
- `clock.rs`: two index sources.
  - `FrameCounterClock` (JACK): extends the u32 counter across its wrap (about 24.8 h at
    48 kHz). Any deviation from `prev + n` sets `DISCONTINUITY` with the exact size.
  - `TimestampClock` (cpal): running frame count. If the capture timestamp moves more than
    half a block beyond what the previous block's length predicts, the estimated missing
    frames are added and `DISCONTINUITY|GAP_ESTIMATED` is set.
- `backend.rs`: the trait, capabilities, `Negotiated`, `ClockRelation`, `StaticLatency`,
  `EventLatch` and `DuplexStream`.
  - `EventLatch` turns asynchronous xrun and config notifications (atomics) into flags on
    the next block.
  - `DuplexStream` owns the consumer, an output-timing ring (`OutputTick`), counters, and a
    type-erased guard that keeps the backend stream alive.
- `output.rs`: silence by default. `EmitLevel` is a typed dBFS value that rejects anything
  above −20 dBFS. The tone goes to output channel 0 only, with a 20 ms fade-in and fade-out
  and a hard clamp at the cap. `DuplexStream::stop` requests the fade, waits for the
  renderer to report silence, then drops the stream. **`--emit` was never run.**
- `cpal_backend.rs`, `jack_backend.rs` (feature `jack`, Linux only), `fake.rs`.
- `bin/audio-duplex.rs`:
  - `--list [--backend cpal|jack|fake|all]` prints JSON capabilities.
  - `--run <s>` prints JSON: negotiated config, block counts, flag counts, index gaps and
    regressions, callback and capture interval jitter, input lag, backend event counters,
    per-channel peak, output callback interval, and playback lead.
  - `--trace` prints flagged or irregular blocks.
  - Input routing: `--inputs 3,1` selects device channels in block order.
- Tests: 13 that need no hardware (ring wrap and alignment, whole-block drop, cross-thread
  lossless transfer, u32 wrap, exact and estimated gaps, level cap, fades, fake end-to-end
  routing, xrun index jump, loopback onset at exactly the delay index, slow-consumer
  overflow), plus device tests that skip cleanly:
  - cpal enumeration;
  - ALSA `null` duplex;
  - JACK, only when a server is running and only with `--features jack`.

## 3. Measured locally (Linux, no sound hardware, no RT privileges)

| Run | Result |
|---|---|
| JACK dummy (`jackd -d dummy -r 48000 -p 256 -C 4 -P 2`), 10 s, 4 in / 2 out, release | 1876 blocks, all 256 frames, **0 index gaps, 0 regressions, 0 drops, 0 xruns**. Wake interval stddev 181 µs, max deviation 3.5 ms (not RT-scheduled). DLL cycle-start interval stddev 0.86 µs. Port latency capture 256 / playback 512 frames. |
| Same, debug build, first 5 s run | 5 xruns, 3 exact index gaps (1024 frames total), all flagged. jackd log: "client was not finished" (non-RT scheduling in this sandbox). |
| cpal ALSA `null`, 3 s | The PCM has no clock, so callbacks spin at about 1.6 µs. This stress-tests the transport: 620 k blocks delivered, 3.9 M dropped. **Every one of the 1350 index gaps carried `OVERFLOW`**, with 0 regressions and channels never misaligned. |

Two observations from the JACK trace shape the trait:

1. **A late JACK cycle need not skip frames.** In one trace the callback woke 8.8 ms after the
   previous one (nominal 5.3 ms), but `last_frame_time` stayed contiguous. On real hardware,
   an ALSA driver xrun loses ADC samples while JACK's frame time counts *cycles*, so the index
   can look contiguous across lost audio. **`XRUN` must break continuity by itself**; it is in
   `BREAKS_CONTINUITY`.
2. **The xrun notification is asynchronous.** It arrives on JACK's notification thread, so
   the `XRUN` flag can land one block after the affected block (seen in the trace:
   `flags=0x2` on the block after the late one). Consumers should treat the block before an
   `XRUN` as suspect too. ac2-audio could instead hold the most recent block back for one
   cycle before publishing it.

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

## 6. Recommended trait shape for `ac2-audio`

```rust
pub trait Backend {
    fn kind(&self) -> BackendKind;
    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError>;      // read-only
    fn open(&self, req: &DuplexRequest) -> Result<DuplexStream, AudioError>;
}

pub struct DeviceCaps {
    id, name, host,
    input: Option<DirectionCaps>, output: Option<DirectionCaps>,  // channels, rate ranges,
                                                                  // buffer range (Option), formats
    duplex_clock: ClockRelation,   // SingleCallback | SameDeviceSeparateCallbacks | Unknown
    latency: StaticLatency,        // PortRanges{capture,playback} | PerCallbackTimestamps | None
}

pub struct DuplexRequest {
    input_device, output_device, input_map: Vec<u16>, output_channels,
    sample_rate: Option<u32>, buffer_frames: Option<u32>, ring_seconds,
    output: OutputSource,   // Silence | Generator(lock-free handle) — never a default tone
}

pub struct DuplexStream {          // concrete, not a trait object
    negotiated: Negotiated,        // what actually opened, incl. clock relation & index exactness
    capture: BlockConsumer,        // headers {start_sample, frames, channels, flags,
                                   //          callback_ns, capture_ns}
    output_ticks: Consumer<OutputTick>, // {start_sample, frames, callback_ns, playback_ns}
    events: Arc<BackendEvents>,    // xruns, errors, config changes (atomics)
    guard: Box<dyn Send>,          // drop = stop; stop() fades first
}
```

Design points that came out of the spike:

1. **Keep the trait object-safe and tiny.** Put the work in shared, backend-agnostic pieces
   that are unit-tested with the fake: `BlockProducer`, the two clocks, `EventLatch` and
   `OutputRenderer`. The real backends turned out to be about 300 lines of glue each.
2. **`Negotiated` must state how exact the index is**: exact frame counter vs estimated.
   Every gap flag also says `GAP_ESTIMATED` when the size is a guess. Jobs reset on any
   `BREAKS_CONTINUITY` flag regardless.
3. **Generator sample index ≠ capture sample index on cpal.** They are separate counters on
   separate threads. Only on JACK, ASIO and fake does a single callback make
   `out_start_sample == in_start_sample` meaningful. Expose `OutputTick` so a job can
   relate them through timestamps, and let the loopback correlation (3a) be the truth.
4. **Ring transport:** two SPSC rings (samples + headers), whole-block-or-nothing, overflow
   carried as a flag. Interleaved is natural for cpal and cheap to produce on JACK; jobs
   that want planar data deinterleave on their own thread. Size the header ring from the
   *smallest* plausible block, because WASAPI packets vary.
5. **One stream per session, fan-out after the ring (PLAN §4.3).** The trait should not
   support multiple consumers in the callback.
6. **Don't use cpal's JACK host.** It creates a client per stream, which loses the
   single-callback property. Use the `jack` crate directly behind a cargo feature, as done
   here.
7. **Later, optional:** thin native CoreAudio/WASAPI backends, or an upstream cpal patch,
   that expose `mSampleTime` / the WASAPI device position. That would make the index exact
   on mac/win as well. It is not needed for 1.0 if the loopback monitor is in place.

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

## 8. Manual hardware verification (cannot be done here)

Run these on one machine per OS, with a real interface and a **physical cable from output 1
to input 1** where the step says loopback. Silence-only steps need no cable. Use a release
build: `cargo run --release -p spike-audio-duplex --bin audio-duplex -- …`. Record the JSON.

**All OSes**

1. `--list --backend cpal`. Check:
   - every interface appears;
   - channel counts match the hardware (WASAPI: does a multichannel interface show all its
     inputs?);
   - rate ranges and buffer ranges look right;
   - `duplex_clock` is `same_device_separate_callbacks` for single-device interfaces on mac
     and Linux, and `unknown` on Windows.
2. Silence run, 60 s, built-in or USB interface: `--run 60 --in-dev <id> --inputs 0,1`.
   Expect `index_gaps = 0`, `flagged_xrun = 0`, `index_regressions = 0`, and all
   `frame_sizes` equal (CoreAudio/ALSA). For WASAPI, write down the distribution. Record
   callback and capture interval stddev, `input_lag_us`, and the output playback lead.
3. Multichannel: `--inputs 0,3,7` on an 8+ input interface. Feed a signal (a mic or an
   external generator) into input 4 only. `channel_peak_dbfs` must show it in block channel 1
   only, with the other channels at the noise floor.
4. Stress: run step 2 while loading the CPU (`stress -c $(nproc)` or a browser benchmark).
   Every xrun must show up as flags. Check `flagged_xrun` and `flagged_discontinuity`
   against the OS's own xrun indication (CoreAudio overload in Console.app; Windows: none
   available).
5. Hot-unplug the USB interface during `--run 30`. Expect `backend_events.errors` or
   `config_changes` > 0 and no crash or hang. Check that `stop()` returns.
6. Change the buffer size from another app mid-run (macOS: Audio MIDI Setup or a DAW;
   Windows: the control panel sample-rate change). Record which flags and errors appear.
7. Restart offset repeatability, the core Q3 risk. **This needs emission; a human runs it,
   not an agent.** Use the loopback cable and turn monitors off:
   `--run 5 --emit -40dbfs`, ten times. The follow-up must compute the output→input offset
   per run. Measure how much it varies between runs on each OS. Expected: varies on cpal,
   constant on JACK.

**macOS specific**

8. Aggregate device of two interfaces: `--list` shows it as one device. A silence run must be
   clean. Note what happens with drift correction on vs off.
9. Microphone permission: the first `--run` from Terminal must prompt. After denial, expect a
   clean `AudioError`, not a hang.

**Windows specific**

10. Pair a capture endpoint and a render endpoint of the same interface (`--in-dev`,
    `--out-dev`). Confirm the stream rate equals the endpoint's mix rate in Sound settings.
    Also try a mismatched `--rate` and expect `UnsupportedConfig`.
11. Callback size distribution at the default period (expect about 480 frames at 48 kHz) and
    with `--buffer 128`. cpal's comment says `Fixed` does not change the period; verify it.
12. Build check: `cargo build -p spike-audio-duplex` with **no** `jack` feature must not need
    libjack.

**Linux real hardware (lower priority; JACK is proven on dummy)**

13. `jackd -d alsa -d hw:X -r 48000 -p 256` and then `--run 60 --backend jack --inputs 0,1`.
    Expect 0 gaps with RT privileges. Then force an xrun (`-p 32` under load). Confirm that
    `XRUN` is set and check whether the frame counter jumps.
14. cpal ALSA on `hw:CARD=X,DEV=0` (expect i32 or i16 native) vs `plughw:`. Compare jitter;
    plughw gets no RT promotion.

## 9. Compared to `ac`

`ac`'s cpal backend took a mutex and allocated in the callback, read only channel 0, was
hard-wired to 44.1 kHz and had no routing. Here, the callbacks only push into rings, open the
device's native format, route arbitrary channel maps, and honour the requested or default
rate. `ac`'s JACK ideas held up and were kept as ideas: SPSC rings, sample-aligned capture,
and port latency ranges as plausibility data. New here: a sample index on every block, and
gap and overflow flags carried in-band rather than as side counters.
