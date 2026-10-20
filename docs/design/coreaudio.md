# Core Audio backend (macOS)

**Status: stage (c) built** (§11: `cahal` and the `coreaudio` backend, the only macOS
backend; cpal removed; the tester round of §10c is still open; decision H1 in
`open-questions.md`). Before this backend, macOS audio went through
cpal (`cpal_host.rs`, removed in (c)): two AUHAL units, one per direction, each
with its own callback, a running frame count for the index (`IndexExactness::Estimated`)
and timestamps that put input against output two buffers off (`macos-ci-audio.md` Q4: a
loop that is 0 samples in the device's own time reads as −2 buffers). The HAL itself gives
what JACK and ASIO give ac2: one IOProc per device that serves capture and playback in one
call, with one `AudioTimeStamp` per side on one sample-time line (`mSampleTime`). On
BlackHole a click written at output sample time T was read at input sample time T, every
click (Q4). This document designs a direct HAL backend that replaces cpal on macOS.

The HAL binding is a crate of its own, **`cahal`** (§1), in its own repository
(github.com/mkovero/cahal, published on crates.io), the way `azo` is for ASIO;
`ac2-audio/src/coreaudio_host/` is a thin adapter from it to the `Backend` trait (§3).

## 1. The `cahal` crate

**Why a crate of its own.** The HAL calls ac2 needs (devices, properties, listeners, one
duplex IOProc with an exact index, overload reporting, a consent check) are what any macOS
audio tool needs, and nothing in them is ac2's. Apart from ac2's policy it can serve other
people, and the unsafe code has one small, reviewable home (§9).

**Name.** `cahal` (Core Audio HAL): free on crates.io (API 404, checked 2026-10-12), no
collision with `coreaudio-rs`, `coreaudio-sys` or `objc2-*` (`halio` is taken).

**Rules for the crate.**
- Its own repository, `mkovero/cahal`, MIT, published on crates.io; ac2 depends on a
  released version (`cahal = "0.1.0"` in `ac2-audio` and `tools/release/coreaudio-loop`).
  **No `ac2-*` dependency** and no ac2 policy: anything only ac2 wants belongs in the
  adapter (§3).
- **Changing it:** a PR in `mkovero/cahal` (its CI: fmt, clippy and tests on Linux for the
  pure part and on macOS for the binding, docs with warnings denied, a publish dry run),
  then a release (version bump, tag `vX.Y.Z`, `cargo publish`), then bump the version here
  in the same commit as the adapter change that needs it. While iterating, a local
  `[patch.crates-io] cahal = { path = "…" }` on a scratch branch tests both sides
  together; it never lands on main.
- The HAL code is `cfg(target_os = "macos")`. Elsewhere the library holds only its pure
  part: the plain data types (`DeviceInfo`, `StreamLayout`, `Latency`, `CycleTime`, `Gap`)
  and the sample-time indexer, no dependencies and no `unsafe`. That is the one departure
  from "an empty lib elsewhere", and it is what lets the indexer and the adapter's mapping
  run in Linux's quick tier (§10).
- Dependencies (macOS only): `objc2-core-audio` 0.3 and `objc2-core-audio-types` 0.3
  (already in `Cargo.lock` at 0.3.2 through cpal), `objc2-core-foundation` 0.3 for
  `CFString`, and, behind the default feature `consent`, `objc2-av-foundation` 0.3
  (0.3.2 on crates.io, new to the lock). Not `coreaudio-sys` (bindgen, needs the SDK
  headers and libclang at build time) and not `coreaudio-rs` (AUHAL, the model this
  replaces). Checked in `objc2-core-audio` 0.3.2: the `AudioObject…PropertyData` calls,
  `AudioObjectAddPropertyListener`, `AudioDeviceCreateIOProcID` / `Start` / `Stop` /
  `DestroyIOProcID`, the host-time functions, and every selector named in this document.

**Public API (sketch).**

```rust
pub enum Scope { Input, Output }

/// The HAL's system object.
pub struct System;
impl System {
    pub fn devices() -> Result<Vec<Device>, Error>;
    pub fn default_device(scope: Scope) -> Result<Option<Device>, Error>;
    pub fn device_by_uid(uid: &str) -> Result<Option<Device>, Error>;
    pub fn listen(props: &[SystemProperty], f: impl Fn(SystemProperty) + Send + Sync + 'static)
        -> Result<Listener, Error>;
}
pub enum SystemProperty { Devices, DefaultInput, DefaultOutput, ServiceRestarted }

/// An `AudioObjectID` that is a device. Copy: the HAL owns the object.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Device(u32);
impl Device {
    pub fn object_id(self) -> u32;
    pub fn info(self) -> Result<DeviceInfo, Error>; // uid, name, transport, streams per scope
    pub fn is_alive(self) -> Result<bool, Error>;
    pub fn layout(self, scope: Scope) -> Result<StreamLayout, Error>;
    pub fn channel_names(self, scope: Scope) -> Result<Vec<Option<String>>, Error>;
    pub fn nominal_rate(self) -> Result<f64, Error>;
    pub fn set_nominal_rate(self, hz: f64) -> Result<(), Error>; // waits for the change
    pub fn available_rates(self) -> Result<Vec<RateRange>, Error>;
    pub fn buffer_frames(self) -> Result<u32, Error>; // also set_…, …_range, uses_variable_…
    pub fn latency(self, scope: Scope) -> Result<Latency, Error>;
    pub fn clock_domain(self) -> Result<u32, Error>;
    pub fn clock_sources(self) -> Result<Vec<ClockSource>, Error>; // current one marked
    pub fn listen(self, props: &[DeviceProperty],
                  f: impl Fn(DeviceProperty) + Send + Sync + 'static) -> Result<Listener, Error>;
    pub fn start_io<F>(self, config: IoConfig, f: F) -> Result<IoProc, Error>
        where F: FnMut(Cycle<'_>) + Send + 'static;
}
pub enum DeviceProperty { Alive, NominalRate, BufferFrameSize, ProcessorOverload,
    Latency, StreamFormat, StreamConfiguration, ClockSource, IoStoppedAbnormally }

pub struct Latency { pub device: u32, pub safety_offset: u32, pub streams: Vec<u32> }
pub struct StreamLayout { pub streams: Vec<StreamInfo> }      // device order
pub struct StreamInfo { pub channels: u32, pub interleaved: bool, pub format: StreamFormat }
pub enum StreamFormat { Float32, Other { format_id: u32, flags: u32, bits: u32 } }

pub struct IoConfig { pub input: bool, pub output: bool }

/// One IOProc cycle. Borrowed from the HAL for the call only.
pub struct Cycle<'a> { pub input: Input<'a>, pub output: Output<'a>, pub time: CycleTime }
pub struct Input<'a>  { /* the AudioBufferList + a channel table built at start */ }
pub struct Output<'a> { /* same, mutable; zeroed before the closure runs */ }
// Both: frames(), channels(); Input::channel(c) -> Channel<'a>,
// Output::channel_mut(c) -> ChannelMut<'_>.
/// One channel inside a (possibly interleaved) stream buffer: `data[offset + i * stride]`.
pub struct Channel<'a> { data: &'a [f32], offset: usize, stride: usize }

pub struct CycleTime {
    pub index: u64,              // shared by input and output of this cycle (§5)
    pub frames: u32,
    pub gap: Gap,                // from the previous cycle
    pub overloads: u32,          // kAudioDeviceProcessorOverload since the previous cycle
    pub input_sample_time: f64,  // raw mSampleTime of inInputTime
    pub output_sample_time: f64, // raw mSampleTime of inOutputTime
    pub now_ns: u64,             // inNow.mHostTime, as ns on the host clock
    pub input_ns: Option<u64>,   // when the first input frame was sampled
    pub output_ns: Option<u64>,  // when the first output frame will play
    pub rate_scalar: f64,
}
pub enum Gap { First, None, Forward(u64), Backward }

/// A running IOProc. Drop stops it; after `stop` returns the closure is never called again.
pub struct IoProc { /* device, proc id, Box<State> */ }
impl IoProc { pub fn stats(&self) -> IoStats; pub fn stop(self) -> Result<(), Error>; }
pub struct IoStats { pub cycles: u64, pub overloads: u64, pub gaps: u64,
                     pub frames_lost: u64, pub panicked: bool }

pub struct Listener { /* unregisters on Drop */ }

#[cfg(feature = "consent")]
pub mod consent {
    pub enum Authorization { Authorized, Denied, Restricted, NotDetermined }
    pub fn microphone() -> Authorization;
    pub fn request_microphone(timeout: std::time::Duration) -> Authorization;
}

pub enum Error {
    Status { call: &'static str, selector: FourCc, status: i32 },
    NotFound, Unsupported(&'static str), Utf8, StartTimedOut,
}
```

The closure is `FnMut + Send + 'static` and is called on the HAL's I/O thread. The crate
promises the closure nothing allocates or locks on its side of the call (§6); what the
closure does is the caller's business, which is why ac2's rules for it live in the adapter.

**What stays out of the crate.** Everything that is ac2's policy: the emit level cap and
the fade (`output.rs`), the block hand-off into the `rtrb` transport, `BlockFlags`, the
event latches, `StaticLatency`, `ClockRelation`, recovery, device ids of the form
`coreaudio:<UID>`, channel naming fallbacks, error texts for the operator.

**Split out.** It began in ac2's workspace so one commit could change the API and the
adapter while both were written. The split criteria were (1) the API unchanged through the
switch (§11b) and one tester round on a real interface, (2) its own CI on a hosted macOS
runner with BlackHole (the IOProc loop test of §10b, without ac2), and (3) a README and
rustdoc covering every public item with an example. It moved to `mkovero/cahal` on
2026-10-12 with (1) met through the switch (the tester round is still open, §10c) and its
own CI building and testing the binding on macOS; the BlackHole IOProc loop test in that
repository and per-item examples remain to do there. Until then ac2's BlackHole pass
(§10b, `coreaudio-loop`) is the only real-device loop test of the IOProc.

## 2. Device model

**One device with both directions** (an interface, BlackHole, a user-built aggregate): one
IOProc on that device. The HAL calls it once per cycle with the input buffers it has read
and the output buffers to fill, and one `inInputTime` / `inOutputTime` pair on the device's
one sample-time line. This is JACK's and ASIO's model: `ClockRelation::SingleCallback`,
`IndexExactness::Exact`.

**Default output.** A request whose output device is `Default` plays on the input device
when that device has outputs, even if the system's default output is another device: one
device, one IOProc, one clock is the point of this backend. Another output device has to be
named.

**Separate input and output devices** (a USB measurement mic beside the built-in output,
the common light setup). The options:

| Option | One callback | Clock | What ac2 can see |
|---|---|---|---|
| A user-built aggregate (Audio MIDI Setup) | yes: it is one device | the aggregate's clock source; other subdevices drift-corrected if the user ticked it, else they slide | one device; `kAudioSubDevicePropertyDriftCompensation` per subdevice |
| A private aggregate ac2 creates (`AudioHardwareCreateAggregateDevice`, `kAudioAggregateDeviceIsPrivateKey`, drift correction on the non-master) | yes | the master's clock; the other side resampled by Core Audio | a resampler whose quality and latency wander ac2 cannot see, and a device ac2 creates and must destroy on every open, reopen and crash path |
| Two IOProcs, one per device | no | two clocks | each side exact on its own line; drift between them is what `multi-device.md` §5 measures |

**Decision: two IOProcs** for two devices, `ClockRelation::Unknown`, each side indexed
exactly from its own `mSampleTime`. Why:
- It is what ac2 already handles and tests: a transfer function compares two inputs on one
  input device, so output timing never touches it (`multi-device.md` §3); the loopback
  monitor already measures drift between output and input clocks and raises CLOCK DRIFT.
- A private aggregate would hide the drift behind a resampler ac2 cannot inspect: the
  monitor would see slow latency wander instead of a drift it can name, and the claim
  "one callback" would be true while "one clock" is not.
- Creating devices is state on the operator's machine. A private aggregate dies with the
  process, but a crash during a reopen, coreaudiod restarts and hot-plug all have to be
  handled for a device ac2 owns, for no measurement gain.
- The operator who wants one clock builds the aggregate in Audio MIDI Setup, where its
  drift correction is visible and theirs. ac2 opens it as one device (`SingleCallback`) and
  lists, in `DeviceCaps::notes`, the subdevices with drift correction off ("two clocks
  inside: … free-runs"). Two devices in one `ClockDomain` stay `Unknown`, noted (§12 Q3).

## 3. Backend shape in `ac2-audio`

**Placement.** `crates/ac2-audio/src/coreaudio_host/`, built on macOS and, for its unit
tests, on every OS (`cfg(any(target_os = "macos", test))`), like `asio_host`:
- `mod.rs`: `CoreAudioBackend` (the `Backend` impl) over a `HalSource` trait.
- `source.rs`: `HalSource`, what the adapter needs from `cahal` (list, look up by UID,
  properties, start an IOProc with a closure, listeners, consent). `CahalSource` (macOS) is
  `cahal`; the tests' `Scripted` is a set of scripted devices that drive the closure with
  synthetic `Cycle`s, as `asio_host`'s `Installed` drives `bufferSwitch`.
- `caps.rs`: `DeviceInfo` → `DeviceCaps` (pure).
- `engine.rs`: the IOProc closure: capture into the ring, render the generator, latch events.
- `open.rs`: open, negotiate, the two-device path, consent, error mapping.
- `#![forbid(unsafe_code)]` in `mod.rs`: the adapter has none (§9).

**Kind and id.** New `BackendKind::CoreAudio` (host `"coreaudio"`). The device id stays
`coreaudio:<UID>`, which is what cpal's id rendered (`cpal::DeviceId` Display is
`<host>:<id>`, the coreaudio id is `kAudioDevicePropertyDeviceUID`), so sessions, the CLI
`--device coreaudio:BlackHole2ch_UID` and stored device ids keep naming the same device. It
is the natural id of a Core Audio device, not a compatibility layer.

**`enumerate`.** One `DeviceCaps` per HAL device with at least one stream:
- `input` / `output`: `max_channels` from the stream configuration; `rates` from
  `AvailableNominalSampleRates` (ranges as given); `buffer_frames` from
  `BufferFrameSizeRange`; `default_buffer` the current `BufferFrameSize`; `default_rate`
  the nominal rate; `formats: [F32]` (§4); `system_default` from the default devices.
- `channel_names` from `kAudioObjectPropertyElementName` per channel (interfaces name
  them, "Analog 1", "ADAT 3"); empty or duplicate names fall back to the numbered form, as
  in `asio_host`.
- `duplex_clock`: `SingleCallback` when the device has both directions, else `Unknown`.
- `index: Exact`. `latency`: `StaticLatency::Frames` (§7).
- `clock_sources`: `kAudioDevicePropertyClockSource` with its names
  (`…ClockSourceNameForIDCFString`), read-only as on ASIO.
- `notes`: a non-Float32 virtual format, variable buffer sizes, an aggregate's
  undrift-corrected subdevices, hog mode held by another process.

**`probe`.** `TranslateUIDToDevice`: no object → `Absent`; an object → `Present` with the
`AudioObjectID` as `generation` (a replugged device normally gets a new object id; when it
does not, the reopen attempts still find it). Opens nothing.

**`control_panel`.** Brings up Audio MIDI Setup (`open -b com.apple.audio.AudioMIDISetup`)
for a device that is present (§12 Q4); a device that is not is `DeviceNotFound`. A rate
changed there reaches a running stream through the rate listener (§4, a reset).

**Selection.** `coreaudio` is the macOS backend of the daemon, CLI (`BackendArg::CoreAudio`)
and the app's embedded daemon (§11b); `--backend cpal` is gone since (c).

## 4. Format and configuration

**Stream format.** The IOProc sees each stream in its **virtual format**, which the HAL
converts to and from the physical one. On current macOS that is 32-bit native float for
every device ac2 has seen; the backend asks for `kAudioFormatLinearPCM`, float, packed,
native endian, and refuses any other virtual format at open with the format named
(`Unsupported::SampleFormat`), noted in `enumerate`. Interleaving is per stream: a stream buffer
holds `mNumberChannels` interleaved channels, and a device can have several streams per
direction (an interface: analog 1–8 in one stream, ADAT 9–16 in another). `cahal` builds a
channel table at start (device channel → stream buffer, offset, stride) so the closure
reads a channel without searching; ac2 never assumes one stream.

**Unused streams.** `kAudioDevicePropertyIOProcStreamUsage` turns off input streams no
mapped channel uses, so the HAL does not convert them. Output streams stay on: the
generator's routes change at run time without a reopen (as in `asio_host`).
The adapter passes the input streams the request's `input_map` touches as
`IoConfig::input_streams` (`StreamLayout::streams_of`); `cahal` sets the property between
create and start, and only when some stream is left out, so a one-stream device (BlackHole)
takes the same path as before. A stream left out arrives with a null buffer and reads as
silence. Setting it is best effort: a device that refuses keeps every stream on, which
costs conversion time and changes no sample read.

**Rate.** `set_nominal_rate` only when the request names a rate other than the current
one; it is a device-wide setting other apps see, so an unrequested rate is never forced.
The set is asynchronous: `cahal` waits for the `NominalSampleRate` listener (≤ 2 s) and
re-reads; a device that will not take it is `Unsupported::SampleRate`. A rate change while
running (another app, Audio MIDI Setup, an external clock) ends the stream with
`events().reset`: the daemon's `driver_reset` path reopens at the device's new settings
and reports `driver_change`, exactly as for an ASIO driver reset (`asio.md` §3a).

**Buffer.** `BufferFrameSize` is per device but the HAL keeps the smallest any client
asks for; ac2 sets the request's size (default `short_buffer_frames`, ~20 ms) within
`BufferFrameSizeRange` and reads it back after start. A device that
`UsesVariableBufferFrameSizes` gets `Negotiated::buffer_frames: None`, sizes the scratch
for the maximum, and is noted. A size change while running (another client asked for a
smaller one) is a config change, not a stop: the index stays exact across it.

**Channels.** `input_map` indexes device input channels across all input streams in device
order; output channels are `0..output_channels`, the rest of the device's outputs get zeros.

## 5. Sample index and discontinuities

**Index.** One counter per IOProc: `index = round(inInputTime.mSampleTime) − origin`, where
`origin` is the first indexed cycle's input sample time. Input and output of the cycle
take the same index, the `SingleCallback` contract that JACK and ASIO keep; the fixed
distance between `inOutputTime` and `inInputTime` goes into the stated latency (§7), not
into the index. An output-only IOProc (two-device case) uses `inOutputTime` the same way.

**Gaps** (`cahal`'s `Indexer`, pure, shared by both directions):

| Sample time against `last + frames` | `Gap` | ac2 flags |
|---|---|---|
| equal | `None` | — |
| ahead by `n` | `Forward(n)`: exact frames lost | `DISCONTINUITY`, index += `n` |
| behind (the device restarted its IO: another client changed the rate, a reset) | `Backward`: index stays monotonic | `DISCONTINUITY` |
| `mFlags` without `SampleTimeValid` | `Forward(0)` once, then counting | `DISCONTINUITY \| GAP_ESTIMATED`, `IndexExactness::Estimated` noted |

A non-integral sample time (a rate scalar off 1 on a varispeed device) is rounded; the
indexer accumulates the remainder so no frame is lost or counted twice.

**Overloads.** `kAudioDeviceProcessorOverload` is a property notification on the HAL's
notification thread, not part of the IOProc. `cahal` registers it with every IOProc and
counts it in an atomic; the next cycle reads and clears it into `CycleTime::overloads`,
and the adapter latches `XRUN` on that block. An overload usually also shows as a
`Forward` gap one cycle later; both flags are kept, as JACK's xrun plus frame-time jump.

**Mapping onto the trait.**

| `ac2-audio` | JACK | ASIO | Core Audio |
|---|---|---|---|
| `ClockRelation` | `SingleCallback` | `SingleCallback` | one device: `SingleCallback`; two: `Unknown` |
| `IndexExactness` | `Exact` (`last_frame_time`) | `Exact` (sample position) | `Exact` (`mSampleTime`) |
| clock in code | `FrameCounterClock` | `PositionClock` | `cahal::Indexer` → `BlockFlags` |
| `StaticLatency` | `PortRanges` | `Frames` (`getLatencies`) | `Frames` (§7) |
| `callback_ns` | jack time | QPC | `inNow.mHostTime` → ns |
| `capture_ns` / `playback_ns` | from the frame time | `None` | `inInputTime` / `inOutputTime` host times |
| `Delivery` | `Device` | `Device` | `Device` |

Host times convert with the mach timebase read once at start (pure arithmetic in the
callback, no `AudioConvertHostTimeToNanos` call).

## 6. Callback rules

The IOProc runs on the HAL's real-time I/O thread; ac2 creates no audio thread. Neither
`cahal` nor the adapter allocates, locks or makes a syscall in it.

- `AudioDeviceCreateIOProcID` gets an `extern "C-unwind"` trampoline and a pointer to a
  `Box<State>` (closure, indexer, channel tables, overload atomic) allocated before
  `AudioDeviceStart` and freed only after `AudioDeviceStop` and
  `AudioDeviceDestroyIOProcID` have returned, after which the HAL never calls it.
- The trampoline builds `Cycle` from the `AudioBufferList`s on the stack, zeroes the output
  buffers, calls the closure inside `catch_unwind`. A panic never unwinds into the HAL:
  outputs stay zero, `IoStats::panicked` is set, the closure is not called again, and the
  adapter ends the stream (`events().end()`).
- The adapter's closure does what `asio_host`'s callback does and nothing else:
  1. map `CycleTime` (gap, overloads) to `BlockFlags`, take latched events;
  2. copy the mapped input channels into the `rtrb` capture ring (`BlockProducer::push_with`);
  3. render the generator through `OutputRenderer` into the output channels;
  4. push the `OutputTick`.
- Scratch is sized for `BufferFrameSizeRange.max` at open, so a buffer change never
  allocates.
- Listeners run on the HAL's notification thread and only bump `BackendEvents` atomics or
  set flags a watcher thread reads; anything that needs a property read (latency changed)
  is done by the watcher (`ac2-ca-watch`), as `ac2-asio-latency` does.
- The drain side is unchanged: `timer::Sleeper`'s `NOTE_CRITICAL` kqueue timer. App Nap
  (backlog) can throttle that thread, never the HAL's; the IOProc keeps filling the 2 s
  ring. ac2 does not join the HAL's `os_workgroup`: the drain is not on the cycle deadline.

## 7. Latency

For one device the HAL places the cycle so that `inInputTime = now − buffer − safety_in` and
`inOutputTime = now + buffer + safety_out`; the device and stream latencies are the
converter and transport on each side (`kAudioDevicePropertyLatency`,
`kAudioStreamPropertyLatency` of the stream used). With the shared index of §5, a sample
written at index `i` is read back through a zero-length analog loop at

```
i + (out_time − in_time) + latency_out + stream_out + latency_in + stream_in
```

The backend states `StaticLatency::Frames { input, output }` so that `input + output` is
exactly that, the ASIO convention (latency including the buffer):

- `input = buffer + safety_in + latency_in + stream_in`
- `output = (out_time − in_time) − buffer − safety_in + latency_out + stream_out`

`out_time − in_time` is read from the first cycle, not computed: on BlackHole it is 2
buffers (latencies and safety offsets 0, `macos-ci-audio.md` Q4), so `input + output` = 2
buffers, which is the loopback the click test of §10b must see. If it changes while
running, or a `Latency` listener fires, the watcher re-reads and latches `CONFIG_CHANGE`
only when the values differ. For the two-device case each side states its own part; their
sum is meaningless because the two indexes have an arbitrary offset.

**What the loopback monitor needs** (`q3-loopback-timing.md`). Nothing new in the
estimator: on one device the offset is within one counter, as on JACK and ASIO, so it is
the same across restarts (cpal's arbitrary start difference goes). Display only: the
offset minus `input + output` is the analog and converter part (≥ 0 and small on a cable;
negative means the device misstates its latency), plausibility data (decision 3a). Two
devices: unchanged, stability and drift are what is measured.

## 8. Lifecycle and recovery

**Device list** is read anew on every `enumerate`. **Hot-unplug:** `DeviceIsAlive` →
false, `IOStoppedAbnormally`, or `ServiceRestarted` (coreaudiod restarted) →
`events().end()` (cause `host_ended`); a device that just stops calling back is caught by
the `not_delivering` bound. Recovery (`audio-recovery.md`) reopens with backoff.

**Default device changes.** `DeviceSelector::Default` resolves to a UID at open and
`Negotiated` carries that UID. A later default change does **not** move a running stream:
a measurement rig must not switch interfaces because someone plugged in headphones. A
recovery reopen resolves the selector again.

**Settings changed by someone else** (rate, clock source, a format change):
`events().reset` → `driver_reset`, reopen at the device's settings, `driver_change` shown
(§4). Buffer size and latency: config change only.

**Hog mode.** A device hogged by another process (`kAudioDevicePropertyHogMode` ≠ −1 and
not ours) refuses with `Unavailable` ("in exclusive use by <pid's name>"); recovery
retries it. ac2 never takes hog mode itself: other apps keep working beside a rig.

**Microphone consent.** Without a TCC grant, capture does not fail or return zeros: the
spike saw `AudioDeviceStart` block forever, and the pending request wedge later captures
in the same login session (`macos-ci-audio.md` Q2). A capture timeout cannot recover from
that: the thread that called `AudioDeviceStart` is gone for good. **Decision: check up
front** with `AVCaptureDevice.authorizationStatus(for: .audio)` (`cahal::consent`, through
`objc2-av-foundation`) before any IOProc with input starts:
- `Authorized` → open.
- `Denied` / `Restricted` → `Unavailable(MicrophoneDenied)`: "macOS denies ac2 the
  microphone: allow it in System Settings › Privacy & Security › Microphone" (for the CLI,
  the terminal app is the one to allow, since TCC charges the responsible process).
- `NotDetermined` → `request_microphone(60 s)` on the open thread (the app's first open
  shows the system prompt), then as above; no answer in time is
  `Unavailable(MicrophonePending)` naming the prompt, and recovery retries with backoff.
As a backstop the open waits for the first IOProc cycle for `max(2 s, 4 periods)` and
otherwise fails with `Error::StartTimedOut`; the start runs on the opening thread, which in
the daemon is the recovery's own `ac2d-reopen` thread, so a hung start never blocks the
control thread. Output-only opens skip the check.

## 9. Emitting safety and unsafe surface

**Emitting.** Output samples come only from `OutputRenderer` (`output.rs`), which every
backend uses: silence unless a generator is armed, the request's `MaxLevel` as the global
cap, the fade-out on stop, counted limiting. `DuplexStream::stop` fades, waits `drain`
(buffer + output latency + safety offset + 20 ms) and only then drops the guard, whose
`Drop` stops and destroys the IOProc. `cahal` zeroes output buffers before every closure
call, so a cycle that the adapter skips (panic, stopping) plays silence. Nothing in
`cahal` emits on its own.

**Unsafe.** All of it is in `cahal` (its own repository, github.com/mkovero/cahal, from
crates.io; no longer in this workspace), in four files there, each block with a `// SAFETY:`
comment (`clippy::undocumented_unsafe_blocks` denied in the crate):
- `property.rs`: typed `get` / `set` / `size` over `AudioObject…PropertyData`, checking the
  returned size against the type; `CFString` reads release what they get.
- `ioproc.rs`: create, start, stop, destroy; the trampoline; building slices from the
  `AudioBufferList` (lengths from `mDataByteSize`, checked against the channel table).
- `listener.rs`: register / unregister with a boxed closure; unregister before free.
- `consent.rs`: the two AVFoundation calls.
The pure part (indexer, layout, types) and the adapter are `forbid(unsafe_code)`. A
reviewer reads four files to audit the backend.

## 10. Tests

**Audio safety.** Every automated stage runs on the scripted source, the fake backend or
BlackHole on a hosted runner (no audio hardware). No automated run passes `--emit`. The one
program that writes nonzero samples (§10b) refuses any device but BlackHole.

**(0) Linux, quick tier.**
- `cahal` (pure part): the indexer on scripted sample times: steady, `Forward(n)` exact,
  backward stays monotonic, invalid flag → estimated, fractional times accumulate exactly,
  overload counts; the channel table for one interleaved stream, several streams,
  non-interleaved, mono streams.
- `coreaudio_host` against `Scripted`: caps mapping (rates, ranges, names and their
  fallback, notes), `StaticLatency::Frames` from latencies, safety offsets and a scripted
  `out_time − in_time`; sparse `input_map` across two streams; unplug → `end`; rate change
  → `reset`; overload → `XRUN` on the next block; a panicking closure → silent, ended;
  consent `Denied` / `NotDetermined`-timeout → the typed `Unavailable`; two devices →
  `Unknown`, separate indexes.

**(a) macOS tests** on every all-OS dispatch build the HAL half; `cahal`'s HAL tests list
devices and read properties (no IOProc).

**(b) macOS CI on BlackHole** (`tools/release/macos-blackhole.sh`, all-OS dispatch), the
existing checks with `--backend coreaudio`, then a new **exact-index loopback check**:
- A small tool, `tools/release/coreaudio-loop` (as `tools/wine-asio/ac2-loop`), opens
  `CoreAudioBackend` directly, refuses any device whose UID is not `BlackHole2ch_UID` or
  whose transport type is not `kAudioDeviceTransportTypeVirtual`, writes −20 dBFS
  single-sample impulses on output 1 at known output indexes T, and finds them on input 1.
- Pass: every impulse read at exactly `T + input + output` from the stated
  `StaticLatency::Frames` (2 buffers on BlackHole), over 20 open/close cycles at 1024 and
  2048 frames, and no other nonzero sample; an impulse inside a flagged gap may be missing,
  never misplaced. This proves the shared index, the gap rule and the latency arithmetic on
  the real HAL in one check.
- Also: `ac2 devices` lists `coreaudio:BlackHole2ch_UID` with `index exact`, `clock
  single_callback`. A last step removes the grant and expects `MicrophoneDenied` /
  `…Pending` within its deadline instead of a hang (last, because a pending request may
  wedge later captures on the runner).
- Every call stays under a deadline, as now; sox still feeds the silent checks.

**(c) Human tester** (`testing/macos/README.md`): the existing tests on the new build, plus
a cable loop out → in with `ac2 selftest duplex --emit …` (the tester types the level):
the loopback delay equals `input + output` plus a small converter remainder and is the
same across 10 stop/starts; a rate change in Audio MIDI Setup mid-run reopens by itself at
the new rate with the `driver changed` note; unplug and replug reopen within the backoff
cap; the one-hour run with 0 discontinuities. Until it passes, the backend ships as preview.

## 11. Staging

**(a) Add `cahal` and the backend beside cpal.** PROTO bump (new `BackendKind::CoreAudio`
on the wire). A kind names the implementation, so the new backend gets its own from the
start instead of running under `Cpal` until a rename; (c) then removes `Cpal`. Both exist
for (a)–(b) so BlackHole CI and a tester can compare them on one build.
- `cahal` (new; first `crates/cahal/`, since moved to its own repository
  github.com/mkovero/cahal): `Cargo.toml`, `README.md`, `LICENSE`, `src/{lib.rs, types.rs,
  index.rs, layout.rs, system.rs, device.rs, property.rs, ioproc.rs, listener.rs,
  consent.rs}`.
- `crates/ac2-audio/Cargo.toml` (`cahal = { path = "../cahal" }` for macOS),
  `src/lib.rs`, `src/backend.rs` (`BackendKind::CoreAudio`), `src/error.rs`
  (`Unavailability::MicrophoneDenied`, `…::MicrophonePending`),
  `src/coreaudio_host/{mod.rs, source.rs, caps.rs, engine.rs, open.rs, tests.rs}`.
- `crates/ac2-proto/src/model/session.rs`, `src/lib.rs` (`PROTO_VERSION`),
  `fixtures/protocol/WIRE_LOCK`, `tools/protocol/fixture_messages.py`,
  `fixtures/protocol/expected/*`, `docs/protocol.md`.
- `crates/ac2d/src/backend.rs`, `src/conv.rs`, `src/main.rs`; `crates/ac2-cli/src/args/mod.rs`
  (`BackendArg::CoreAudio`); `crates/ac2-ui/src/prefs/file.rs` (`device_key`),
  `src/embedded.rs`.
- `tools/release/coreaudio-loop/` (new), `tools/release/macos-blackhole.sh` (both backends,
  the loop check on `coreaudio`).
- `CLAUDE.md` crate map: a `cahal` row ("Core Audio HAL binding, macOS: devices,
  properties, listeners, one duplex IOProc with an exact index; no ac2 deps, MIT, to be
  split out"); `STATUS.md`.

**(b) Switch macOS to it.** No wire change. **Built**: also `IOProcStreamUsage` (§4) and
Audio MIDI Setup as the control panel (§12 Q4); the BlackHole checks run on the default
backend, ac2d started without `--backend`.
- `crates/ac2d/src/backend.rs` (default on macOS), `crates/ac2-cli/src/args/mod.rs`,
  `crates/ac2-ui/src/main.rs` / `embedded.rs` (default backend).
- `tools/release/macos-blackhole.sh` (coreaudio only for the session checks),
  `.github/workflows/ci.yml` if step names change.
- `testing/macos/README.md` (§10c), `docs/install.md`, `docs/user-guide.md`,
  `docs/design/multi-device.md` §2, `docs/design/q3-loopback-timing.md` (§7),
  `docs/design/macos-ci-audio.md` (pointer to this doc), `STATUS.md`.

**(c) Remove cpal.** PROTO bump (`BackendKind::Cpal` leaves the wire), as W1 removed
WASAPI (`6dad3701`, `2b844164`, `57fcd527`). **Built** (PROTO 48): `cpal` is out of the
dependency graph on every target; `ClockRelation::SameDeviceSeparateCallbacks` (wire too)
and `StaticLatency::PerCallbackTimestamps` left with it; `TimestampClock` stays, since
`asio_host` estimates the index from timestamps when a driver gives no sample position.
Old `cpal/<id>` prefs keys and `backend: cpal` sidecars no longer parse (§12 Q1, Q2).
- `crates/ac2-audio/Cargo.toml` (drop `cpal`), `src/cpal_host.rs` (deleted), `src/lib.rs`,
  `src/backend.rs` (`Cpal`, `ClockRelation::SameDeviceSeparateCallbacks` and
  `StaticLatency::PerCallbackTimestamps` if nothing else uses them, `clock.rs`'s
  `TimestampClock` likewise), `src/events.rs` (doc).
- `crates/ac2-proto/src/model/session.rs`, `src/samples/*`, `PROTO_VERSION`, WIRE_LOCK,
  fixtures, `docs/protocol.md`.
- `crates/ac2d/src/backend.rs` (`BackendChoice::Cpal` and its per-OS refusals),
  `src/conv.rs`; `crates/ac2-cli/src/args/mod.rs`, `src/cmd/{devsel,session,selftest}.rs`,
  `tests/it/parse.rs`; `crates/ac2-ui/src/{connect.rs, embedded.rs, main.rs,
  prefs/*, session_dialog/mod.rs}` and their tests; `crates/ac2-traces` raw sidecar tests.
- `tools/release/macos-blackhole.sh`, `tools/wine-asio/` (`cpal_probe`), `Cargo.lock`.
- `CLAUDE.md` crate map (`ac2-audio` row: "jack (Linux), coreaudio (macOS), asio
  (Windows)"), `README.md`, `PLAN.md`, `docs/design/open-questions.md` (L1, H1 done),
  `docs/design/asio.md` ("cpal is macOS only" lines), `docs/design/spike-audio-duplex.md`.

## 12. Open questions

1. **Remembered UI prefs** are keyed `cpal/<id>` (`UiPrefs::device_key`); after (b) the
   same device is `coreaudio/<id>`, so remembered outputs and session defaults are not
   found once. Recommended: accept it (pre-1.0, no shims), say it in the tester guide's
   release notes; the device id itself is unchanged. **Accepted** (the tester guide says
   it; no key migration).
2. **Raw recordings from before (c)** carry `backend: cpal` in their sidecar, which no
   longer parses. Recommended: accept it; only tester recordings exist on macOS. If one
   matters, convert its sidecar by hand. **Accepted** in (c).
3. **Two devices in one clock domain** (same non-zero `ClockDomain`): claim one clock?
   Recommended: keep `Unknown` with a note; the loopback decides (decision 3a).
4. **`control_panel`**: open Audio MIDI Setup (bundle `com.apple.audio.AudioMIDISetup`)?
   **Done in (b)**, in the adapter, not in `cahal`; the reset path already follows what is
   changed there. Clock source stays read-only, as on ASIO.
5. **Consent for a headless daemon** (ssh, launchd): TCC charges the responsible process
   and no prompt can appear. Recommended: `NotDetermined` times out to `MicrophonePending`
   naming the responsible app; the rig guide grants it once by hand.
6. **Real interfaces' latency properties**: BlackHole reports 0 everywhere, so whether
   vendors' `Latency` + stream latency match the measured loop is unknown. Recommended:
   §10c records the remainder per interface; if it is ever negative, the stated latency is
   marked unreliable in `notes` for that device, not corrected.

### Settled on CI (stage a)

From the all-OS run [38271062157](https://github.com/mkovero/ac2/actions/runs/38271062157)
(`macos-latest`, BlackHole 2ch, 48 kHz), `tools/release/macos-blackhole.sh` with
`coreaudio-loop`:

- **Exact index.** Impulses on output 1 returned on input 1 at exactly the stated latency,
  every one, over 20 opens at each size: 2048 frames stated 4096 (in 2048 + out 2048),
  measured 4096 for all 354 impulses; 1024 frames stated 2048, measured 2048 for all 366.
  No stray sample on either input, no index break. A later run
  ([38285173131](https://github.com/mkovero/ac2/actions/runs/38285173131)) had one VM
  overload (a block flagged `XRUN | DISCONTINUITY`), and the one impulse whose round trip
  spanned it came back 256 frames early while every other one stayed exact: across a flagged
  break the output placement of that cycle is not the stated one. `coreaudio-loop` reports
  such round trips apart and judges only the rest; a per-cycle check of out − in against the
  stated latency in the engine would catch it directly (stage b). The stated latency is the 2 buffers of
  §7 (BlackHole's own latency and safety offsets are 0).
- **cpal beside it, same build.** Self-test and a 30 s session on both: 0 xruns, gaps and
  discontinuities on both, delay finder 480 samples (accepted, PSR 73 dB) on both; cpal
  `same_device_separate_callbacks` / `estimated`, coreaudio `single_callback` / `exact`.
- **Does the microphone request hang?** No, the call returns, but nothing ever answers it.
  With the runner's grant revoked (TCC rows deleted, tccd restarted) the status is
  `NotDetermined`; `requestAccessForMediaType` returned at once, its completion handler
  had not run when the 10 s bound expired, and the status stayed `NotDetermined`: a
  headless process gets no prompt. So the bound is what ends the wait, as designed (§12
  Q5), and a silent open with a 5 s consent wait was refused in 5.03 s with
  `MicrophonePending` instead of hanging in `AudioDeviceStart`.
- **Does a replugged device get a new object id?** Not testable on BlackHole (it cannot be
  unplugged). What CI shows: the object id (54) stayed the same across 40 open/close cycles,
  so `probe`'s generation does not move on a reopen alone. The replug question stays for
  the tester round on a real interface (§10c).
