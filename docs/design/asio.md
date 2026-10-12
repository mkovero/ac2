# ASIO backend (Windows)

**Status: implemented, and the only Windows audio backend** (decision W1 in
`open-questions.md`). `crates/ac2-audio/src/asio_host/` builds on every Windows build, over
the `azo` fork (§1); there is no cargo feature, and cpal is macOS only. It passes under Wine
on wineasio (§4a); FlexASIO on Windows CI (§4b) and real interfaces (§4d) are pending.
§3 is the design; §3a lists where the code settles or departs from it. Shared-mode WASAPI,
which ac2 used on Windows before, has two `IAudioClient`s on two threads, a same-clock
relation that the API cannot prove, and render xruns that are never reported
(`spike-audio-duplex.md` §1). ASIO is the Windows path where one driver callback
(`bufferSwitch`) serves capture and playback on one clock, like JACK. A device without its
own ASIO driver (a USB measurement mic, a class-compliant interface, built-in audio) runs
through FlexASIO (<https://github.com/dechamps/FlexASIO>), an ASIO driver over the Windows
audio stack; with no ASIO driver installed at all, device listing and session open say so
and point at FlexASIO (`Unavailability::NoAsioDriver`). PLAN.md §12, decision 1, leaves the
licence open; §1 below settles it.

## 1. Licensing

**Facts (checked 2026-10-11).**
- Steinberg's SDK download (`https://www.steinberg.net/asiosdk`, which redirects to
  `https://download.steinberg.net/sdk_downloads/ASIO-SDK_2.3.4_2025-10-15.zip`, SHA-256
  `d5ebf0c20dd2c5f43771fd0c1418f4b361bf52434ee670097cfa6b3a335e2eca`) is **dual-licensed**
  from 2.3.4 on. The `changes.txt` entry reads "new DUAL licensing: Proprietary and GPVv3"
  [sic], and `LICENSE.txt` says "licensed under the terms of the Steinberg ASIO License, or
  alternatively under the terms of the General Public License (GPL) Version 3". The change
  was announced on 2025-10-15 alongside the OBS sponsorship
  (<https://www.heise.de/-10963451>,
  <https://www.recordingmag.com/news/new-asio-open-source-license-paves-way-for-obs-partnership/>).
- **Proprietary option.** You need a licence agreement signed by Steinberg before
  publishing. The SDK "may not be distributed in parts or its entirety". There are no fees,
  and Steinberg can end the agreement with 36 months' notice. Under that agreement the
  logo rules of the *ASIO Usage Guidelines* (shipped in the SDK zip) apply in full: the logo
  in the About box or in every ASIO dialog, on every web page that names the product, and
  in the docs.
- **GPLv3 option.** The SDK README says "Trademark usage … is optional under GPLv3, but, if
  used, must comply with Steinberg's official trademark rules", and "Non-compliance with
  logo guidelines does not affect your GPLv3 rights". Any binary that links SDK code
  (`asio.cpp`, `asiodrivers.cpp`, `asiolist.cpp`) is then GPLv3 as a whole.
- **cpal's `asio-sys` (0.4.0; 0.5.0 on cpal master)** runs bindgen over the SDK headers and
  `cc`-compiles the SDK `.cpp` files. It needs LLVM/libclang. It takes the SDK from
  `CPAL_ASIO_DIR`, or otherwise downloads it at build time from the unpinned
  `steinberg.net/asiosdk` with no hash check. Its `asio_import` module (the raw API) is
  `pub(crate)`.
- **`azo`** (<https://github.com/LastExceed/azo>, crates.io 0.4.0, MIT, first release
  2026-07) talks to the drivers' COM objects directly and uses no SDK. It exposes the
  driver's sample position. It is proposed as cpal's next ASIO layer
  (<https://github.com/RustAudio/cpal/issues/1299>). It is young: one author, a few weeks of
  releases.

**Decision.**
1. ac2's source stays MIT, and so does the Windows build: azo links no Steinberg code, so
   the one MSI carries the ASIO backend under MIT.
2. The binding is chosen by a short spike against wineasio (§4a). The spike checks
   enumeration, buffer creation, `bufferSwitchTimeInfo` with a valid sample position,
   messages, and a clean stop/restart. ac2 keeps one binding, never two.
   **Result: azo passes** (`tools/wine-asio/`, azo 0.4.0 under Wine 10 + wineasio 1.3.0). It
   builds for msvc and gnu with no SDK; the sample position steps by exactly one buffer on
   every switch (938/938) and equals `getSamplePosition`; a 2-in/2-out JACK loopback delays
   by exactly one period in every pulse across separate runs. wineasio's system time comes
   from `timeGetTime()` (1 ms steps), so it is unusable for the index and only the sample
   position is used. **azo is the binding.** Quirks: the clock-source name has junk after its
   NUL, `channel_info` reports inactive until buffers exist, the driver handle is `!Send`.
   Had azo failed, the fallback was ac2's own binding over a pinned SDK, shipped as a
   GPLv3 build; with azo that branch is moot.
   **The fork.** ac2 builds azo from <https://github.com/mkovero/azo>, branch `ac2`,
   pinned by commit in `crates/ac2-audio/Cargo.toml` (git dependency, Windows only). It
   starts at upstream azo 0.4.1, which posts host commands as `WM_APP`: `WM_USER` on the
   apartment thread also arrives from the system, and its `wParam` was read as a command
   (a FlexASIO soak aborted on it). Upstream is maintained and answered within a day, so
   each fork change is offered upstream; the fork carries what is not merged yet: the host (COM)
   thread can be named; a channel or driver name that fills its whole fixed buffer is read
   instead of panicking; `Proxy::callback_handle` gives the callback a `Send + Sync` handle
   for `getSamplePosition` / `outputReady` without a round trip to the host thread; and a
   driver is created with its CLSID as the IID, as the SDK's host code does, so no
   reference is released before `init`. wineasio 1.3.0 reads uninitialised state in
   `Release` before `init`: with azo's `IUnknown`-then-query creation, the second driver
   load in one process (enumerate, then open) double-freed inside wineasio. Commands go to
   a message-only window rather than the thread, because a modal loop on the apartment
   thread (a driver's control panel) dispatches window messages and drops thread messages.
   Drivers are held in `Rc`, since wineasio tears an instance down on every `Release`, not
   only the last. Clock source names are cut at the first NUL.
   **azo-sys provenance.** `azo-sys` is a Rust re-declaration of the ASIO interface, not a
   copy of the SDK header: its identifiers are its own (`PCM_I16_LSB` for
   `ASIOSTInt16LSB`, `IIASIORedecl`), its comments are its own, and it uses `bitflags` and a
   macro for C-style enums; no SDK comment text appears in it. The values, struct layouts,
   field order and magic numbers equal `asio.h`, as any binary-compatible declaration must:
   those are interface facts. azo's README states it does not use the SDK.
3. ac2 does not take the proprietary route. A signed agreement, a ban on redistributing the
   SDK, and mandatory logo placement are a poor fit for an MIT project with no legal entity.
4. ac2 uses no ASIO logo. The UI and docs name "ASIO" in plain text only, to describe what
   the build is compatible with. "ASIO" is never part of ac2's product name.

## 2. How cpal's ASIO host works, and why ac2 bypasses it

Sources read: `cpal-0.18.2/src/host/asio/{mod,device,stream}.rs` and
`asio-sys-0.4.0/src/bindings/mod.rs`.

- **Device = driver.** The SDK loads one driver per process (`static GLOBAL_ASIO`).
  `Devices` loads each driver in turn just to list it. Opening a second device unloads the
  first.
- **One `bufferSwitch` for both directions.** `asio-sys` registers one set of
  `ASIOCallbacks`. Its `buffer_switch` is a fallback for ASIO 1 drivers: it calls
  `ASIOGetSamplePosition` and forwards to `buffer_switch_time_info`. That function locks a
  **global `Mutex<Vec<callback>>`** and runs every registered closure in registration
  order. A cpal input stream and a cpal output stream are two closures in that `Vec`. Their
  order is whatever order they were built in, which the API does not promise.
- **Building the second stream rebuilds the first.** `prepare_input_stream(output, …)`
  stops the driver and calls `ASIODisposeBuffers`, then calls `ASIOCreateBuffers` again over
  input and output together. The channels are always `0..n`, so no other subset can be
  chosen (cpal issue 1301).
- **Negotiation.** Rate: `ASIOCanSampleRate`, then `ASIOSetSampleRate` when it differs. A
  fixed buffer size is checked against `ASIOGetBufferSize` (min, max and granularity); the
  default is the driver's preferred size. The sample type is the type of channel 0
  (`ASIOGetChannelInfo`). cpal accepts the Int16, Int24, Int32, Float32 and Float64 types,
  LSB and MSB. It rejects the `Int32LSB16/18/20/24` justified types.
- **In the callback.** It locks the stream mutex (uncontended). It skips a repeated
  `buffer_index`, because some USB drivers (Focusrite, ReaRoute) call twice per period. On
  a driver buffer-size change it **reallocates** the interleave buffer. It reads time only
  as `system_time` (ns). `CallbackInfo` has **no sample position**: the binding computes
  one and discards it (the code has a TODO for it).
- **Messages.**

  | ASIO message | What cpal does |
  |---|---|
  | `kAsioResetRequest`, `kAsioResyncRequest`, sample rate changed | `StreamInvalidated`, delivered from a debounce thread after 500 ms of quiet; ignored while the stream is starting, because some drivers send spurious requests during `ASIOStart` |
  | `kAsioOverload` | `ErrorKind::Xrun` |
  | `kAsioLatenciesChanged` | latency re-read into an atomic |
  | `kAsioBufferSizeChange` | buffer size patched in place; the stream keeps running |

**Verdict: cpal does not give ac2 an exact shared `start_sample`.** It hides the sample
position, holds locks and allocates in the callback path, rebuilds buffers when the second
direction opens, and limits channels to `0..n`. Patching `asio-sys` to carry
`sample_position` would fix only the first of these. ac2 therefore drives the binding
directly and installs **its own** `ASIOCallbacks`.

**Exact index.** `bufferSwitchTimeInfo` delivers `timeInfo.samplePosition` (64-bit, made of
two 32-bit halves). The flag `kSamplePositionValid` is "must always be valid" per
`asio.h`. ASIO 1 drivers call only `bufferSwitch`; there ac2 calls `ASIOGetSamplePosition`
itself, as the SDK's `hostsample.cpp` does. Both directions of one switch get the same
`start_sample = samplePosition − origin`, with the origin taken at the first callback after
`ASIOStart`. A jump other than `+bufferSize` sets `DISCONTINUITY` with its exact size, the
same as `FrameCounterClock` on JACK. Counting callbacks would miss periods the driver drops
internally, so the count is a check, not the source. A driver whose position is invalid or
stuck falls back to the running count with `GAP_ESTIMATED`, and is reported as `Estimated`.

## 3. Backend shape in `ac2-audio`

**Placement and types.**
- Windows only (`cfg(windows)`, no feature), in `crates/ac2-audio/src/asio_host/`.
- The callback logic is plain Rust, so Linux tests build it: format conversion, the
  sample-position clock, message → event mapping, duplicate-index guard. A `driver.rs`
  holds the only code that touches COM or the SDK.
- New `BackendKind::Asio` (host `"asio"`). The protocol enum carries it in every build, so
  an MIT app can talk to a daemon running the ASIO build. This changes the wire, so
  `PROTO_VERSION` is bumped.

**`enumerate`.**
- One `DeviceCaps` per driver listed under `HKLM\SOFTWARE\ASIO`. Input and output come from
  the same driver.
- `duplex_clock: SingleCallback`, `index: Exact`.
- `latency`: a new frames-valued variant read from `ASIOGetLatencies`, re-read on
  `kAsioLatenciesChanged` and latched as a config change. `PortRanges` names JACK ports and
  is not reused.
- `rates`: `ASIOCanSampleRate` over the standard list.
- `buffer_frames`: min..max with the granularity (−1 means powers of two).
  `default_buffer` is the preferred size.
- Loading a driver to enumerate it can take it away from its control panel or another app.
  While ac2's own session holds a driver, every other driver is listed by name only, with a
  note, because the SDK has one driver per process.

**`probe`.** A registry lookup only: the driver's key present → `Present`. It loads nothing.

**`open`.**
- Input and output must name the same driver. A request that pairs an ASIO driver with
  another device is refused: there is no second clock to resample against.
- ac2 creates buffers only for the channels in `input_map` and the generator's routes.
  ASIO buffers are per channel and channels are mono, so no stereo pairing is invented.
- Each channel is converted from its own `ASIOSampleType`. Supported: Int16, Int24 (packed),
  Int32, the `Int32LSB16/18/20/24` types, Float32 and Float64, all LSB. The MSB types exist
  only on old Mac drivers; they are refused with a clear error.

**Channel names (names, not numbers).** `ASIOGetChannelInfo` gives each channel a name of up
to 32 bytes ("Analog 1", "ADAT 3", "AN 1/2 L"). These fill `DirectionCaps::channel_names`,
as JACK port names do, so the input list and roles show the driver's words until the
operator names the channel. When a driver returns empty or duplicate names, the backend
falls back to the numbered form the cpal host already uses.

**Callback rules** (no allocation, locks or syscalls).
- `bufferSwitchTimeInfo` is an `extern "C"` function with no user pointer. It reaches the
  running stream through one `static AtomicPtr` to preallocated state. The pointer is set
  before `ASIOStart` and cleared only after `ASIOStop` has returned.
- Scratch buffers are sized for the driver's maximum buffer size at open, so a size change
  never allocates.
- The callback does the following and nothing else:
  1. Read the position.
  2. Convert the selected inputs into the `rtrb` ring.
  3. Render the generator (silence unless armed; the global level cap and fade from
     `output.rs`) into the output buffers.
  4. Call `ASIOOutputReady` when the driver supports it.
  5. Latch events.
- The ASIO 1 fallback calls `ASIOGetSamplePosition` from inside the callback. That is a
  driver call, accepted because the SDK's own host sample does it.
- **Thread affinity.** Drivers are COM objects. Many expect every non-callback call to come
  from the thread that loaded them. One `ac2-asio` control thread owns the driver and
  serves open, close, reset and enumerate over a channel. The recovery threads
  (`ac2d-close`, `ac2d-reopen`) send requests to it and never touch the driver themselves.

**Messages and recovery** (`audio-recovery.md`).

| ASIO message | What ac2 does |
|---|---|
| `kAsioSelectorSupported` | answers yes for the messages handled here, `kAsioSupportsTimeInfo` and `kAsioEngineVersion` (2) |
| `kAsioResetRequest`, `kAsioResyncRequest`, `kAsioBufferSizeChange`, `sampleRateDidChange` to another rate | the stream **ends** (`EventSnapshot::ended`, cause `host_ended`). The existing recovery reopens the same configuration, which means `ASIOStop` → `DisposeBuffers` → `Exit` → `Init` → `CreateBuffers` → `Start` on the control thread. The epoch changes, jobs restart, and the generator stays disarmed. |
| `kAsioOverload` | an xrun latch on the next block. At open ac2 asks `kAsioCanReportOverload` and records the answer in `notes`, because a driver that cannot report an overload makes "0 xruns" meaningless. |
| `kAsioLatenciesChanged` | re-read the latencies; latch a config change |

- Requests the driver sends during `ASIOStart` (until the first callback) are dropped, as
  cpal does.
- Requests are acted on when they arrive, not after a debounce: the recovery backoff already
  absorbs a burst of them.
- A driver that stops calling `bufferSwitch` is caught by the existing `not_delivering`
  bound (`Delivery::Device`).

**Selection.** `asio` is the Windows backend of the daemon, the CLI (`BackendArg::Asio`)
and the app's embedded daemon; the tester guide is `testing/windows/README.md`.

## 3a. As built

Where the code settles an open point of §3 or departs from it, and why.

- **Driver thread.** azo's `Host` is the `ac2-asio` thread: a COM single-threaded apartment
  with a message pump, which drivers that post window messages to themselves need. Every
  driver call except the two the callback makes goes through it.
- **Callback reach.** One static slot holds the running engine: an `AtomicPtr` plus a user
  count, so clearing it after `stop()` waits until no callback is inside. The callback's
  driver calls (`getSamplePosition`, `outputReady`) go straight to the COM object through
  azo's callback handle: the host thread is blocked in `start()` during the priming switch,
  so a round trip through it would deadlock.
- **Priming switch.** Drivers run one switch on the thread calling `start()` to fill the
  output (wineasio does). It gets silent outputs and no index; index 0 and the position
  origin are the first switch after `start()` returns. Requests arriving before then are
  dropped.
- **Index.** `samplePosition` from the time info, else `getSamplePosition`. A step of one
  buffer is continuous; a larger forward step is an exact `DISCONTINUITY` of that size; a
  backward or partial step is a `DISCONTINUITY` with the index kept monotonic. A switch with
  the same half and position as the last is a duplicate and skipped. With no valid position
  the index is a running count and `IndexExactness::Estimated`, with gaps estimated from
  the system time. Which one a driver gives is decided at open by waiting for its first two
  switches (`max(500 ms, 4 periods)`).
- **Host timestamps.** `callback_ns` is QPC. `capture_ns` / `playback_ns` stay `None`: the
  ASIO system time is on another clock and, under wineasio, in 1 ms steps.
- **Buffer-size change.** ac2 answers `kAsioSelectorSupported(kAsioBufferSizeChange)` with
  *no*, so a driver falls back to `kAsioResetRequest`; either ends the stream. Reopening
  takes the new preferred size.
- **Clock events cut the output.** `kAsioResetRequest`, `kAsioResyncRequest`,
  `kAsioBufferSizeChange` and every `sampleRateDidChange` (the stream's own rate included:
  a relock or another source at the same nominal rate has still moved the sample grid; 0
  is `clock_lost`) are clock events. On one, from whatever thread the driver sends it,
  the engine first cuts the output (`OutputShared::cut`: the next rendered block fades
  out over 2 ms, `CUT_SECONDS`, a ramp rather than a step's click, and the stream's output
  stays silent for good), then records the reason (`ResetReason`, the first one wins) and
  ends the stream. A generator on a clock that moved plays at a wrong rate or through a
  slip; 20 ms of the usual fade is too long for that. Only atomics: no allocation, lock or
  call in the callback. The daemon disarms the generator when the stream stops and never
  re-arms it: emitting again takes a start by the operator. Requests from inside `start()`
  or before the first indexed switch are still dropped. ASIO has no message of its own for
  a change of the current clock source; drivers signal it with a reset or resync request,
  or `sampleRateDidChange`, which this covers.
- **Driver reset.** Each clock event ends the stream with `events().reset` set to its
  reason: the driver changed its own settings (its control panel, a clock it follows).
  The daemon reports that as `StopCause::DriverReset { reason }` and its recovery reopens
  **at the driver's settings**: the rate
  and buffer size the session asked for are dropped for that reopen, and a session that
  pinned them is pinned to the new ones afterwards. Asking for the old rate again would
  undo the operator's change in the panel, or, on a driver following word clock at the new
  rate, fail on every attempt. Everything that depends on the rate is derived again by the
  reopen as for any session open: measurements restart (their FFT bins, smoothing, Leq
  windows and sweeps are built for the new rate; a stored trace keeps its own frequencies),
  delays in samples are re-derived from their seconds, the generator stays disarmed. The
  session carries `driver_change` (the old rate and buffer) until a client opens a session
  anew; the app's top bar shows it in the warning colour (`… · 44.1 kHz · 512 frames ·
  driver changed from 48.0 kHz`), `ac2 session status` as a `NOTE` line. Every
  driver-reset reopen also carries `driver_reset` (reason, and whether the generator was
  emitting), shown the same way: `Audio clock changed (driver asked to resync): output
  stopped` as a `NOTE` line, in the session line's detail and, at the moment it happens, as
  an app toast (a fault when the output was cut, a warning otherwise); the AUDIO STOPPED
  banner meanwhile reads `audio clock changed at 20:36 (driver asked to resync)`. Changing the rate
  under the operator without saying so would make every reading after it silently
  different; refusing to follow it would leave the rig dead until a client noticed.
- **Control panel.** `session.control_panel` (`ac2 session panel`, **P** in Settings ›
  Audio & I/O) calls `ASIOControlPanel` on the `ac2-asio` thread: on the stream's own
  driver while a session runs, otherwise on a driver loaded for it, which stays loaded
  (its panel may be a window of that instance) until a stream or a listing needs the
  process's one driver. Another driver than the running stream's is refused. A modal panel
  runs its own message loop on that thread: azo's command channel is a message-only window
  (`HWND_MESSAGE`), whose messages a modal loop dispatches, so other driver calls still
  get through while the panel is up, where thread messages would be lost. The daemon
  answers once the driver returns, or after 1.2 s with `ack` while a modal panel stays up.
  Under wineasio (`ac2.sh`) the panel answers at once with no stream and over a running
  one, and the stream runs on; wineasio's panel is a separate program (it vforks
  `wineasio-settings`), so the modal case is unseen until a vendor driver.
- **Typed refusals.** `ASIOInit` returns a bool; `getErrorMessage` says why. Mapped once in
  `open.rs` (`refusal`), never by matching text:

  | driver says | step | error | text |
  |---|---|---|---|
  | `ASE_NotPresent` | any | `AsioHardwareNotPresent` | `<driver> is installed but its interface is not present (connect it and switch it on); the driver says: <its text>` |
  | init false, a message | init | `AsioHardwareNotPresent` | as above, with the driver's message |
  | init false, no message | init | `AsioDriverInUse` | `<driver> is in use by another application (close the DAW or other audio program using it)` |
  | `ASE_HWMalfunction`, `ASE_NoMemory` | createBuffers, start | `AsioDriverInUse` | as above |
  | anything else | createBuffers, start | `Backend` | `refused to make buffers: <code>: <text>` |

  Drivers that are exclusive refuse a second client at `init` with no message, or at
  `createBuffers` with a hardware error; one that names its trouble is quoted. Both
  variants are `Unavailable`, so the daemon's recovery retries them with its backoff.
- **Unplug and replug.** An unplugged interface ends the stream (the driver stops calling
  back: the fanout's not-delivering bound, or a reset message). `probe` reads the registry
  only (loading a driver to look could take it from its panel or another program), so the
  device stays "present" and recovery keeps reopening with backoff; while it is away
  `init` or `createBuffers` refuses (`NotPresent`), and the first attempt after it is back
  opens. A replug is found within the backoff cap, 30 s.
- **Clock sources** (read-only). `ASIOGetClockSources` at listing (`DeviceInfo.
  clock_sources`, the current one marked) and at open (`OpenSession.clock_source`).
  Names are cut at the first NUL (drivers leave junk after it in the fixed 32-byte field;
  fixed in the azo fork) and trimmed; an empty name reads `Clock N`. Choosing a source is
  left to the driver's panel: it is a property of the rig (who is master), and a change
  of source resets the driver, which the reset path above already follows.
- **Sparse input maps.** `createBuffers` gets exactly the mapped inputs (deduplicated),
  in device order, and each block channel is filled from its own buffer: inputs 3 and 7
  are two buffers, not seven.
- **Backend structure.** `AsioHost<S: DriverSource>` holds the process's one driver
  (stream or panel) and does listing, opening and the panel; `AzoSource` is the registry
  and COM, the tests' `Installed` a set of scripted drivers, so busy, not-present,
  reset-with-new-rate, unplug/replug, panel and sparse maps are tested headless on Linux.
- **Latency.** `kAsioLatenciesChanged` sets a flag that a small watcher thread
  (`ac2-asio-latency`) reads; it re-reads the latencies and latches `CONFIG_CHANGE` only
  when they differ. wineasio sends the message on every JACK connection change, with
  unchanged values. `StaticLatency::Frames { input, output, driver_buffering }` carries
  them; the session states them as `OpenSession.stated_latency` (`ac2 session status`, the
  app's session-line detail): `stated latency: 288 frames in / 320 out (driver buffering:
  32 frames in / 64 out)`.
- **Driver buffering.** `future(kAsioGetInternalBufferSamples)` (ASIO 2.3) is asked at
  every open, after `createBuffers` (it may depend on the size), so a reopen after a reset
  asks again. The SDK defines `ASIOInternalBufferInfo` as the driver's internal buffering
  *included in* `getLatencies`, so it is shown as the part of the stated latency spent
  inside the driver (device DSP, a USB transfer queue), not added on top: adding it would
  count it twice. A driver that does not answer (`ASE_NotPresent` from drivers before 2.3)
  reads `driver buffering: not reported`, never 0.
- **Buffers.** Inputs: the request's `input_map`, deduplicated (two block channels may carry
  one device channel). Outputs: every channel `0..output_channels`, not only the generator's
  routes, because the routes change at run time without a reopen.
- **Formats.** Int16, Int24 (packed), Int32, Int32LSB16/18/20/24, Float32, Float64 (all
  LSB). Conversion is checked against analytic bit patterns in the unit tests: there are no
  refgen PCM vectors. MSB types are refused at open with the type named, and noted in
  `enumerate`.
- **Separate devices.** A request naming two drivers is `Unsupported::SeparateDevices`.
- **Only backend.** A Windows daemon offers `asio` alone (decision W1), as Linux offers
  `jack` alone; `--backend cpal` on Windows is refused with a pointer to `asio` and
  FlexASIO.
- **No driver.** An empty `HKLM\SOFTWARE\ASIO` is `Unavailable(NoAsioDriver)`: the daemon's
  backend availability, `ac2 devices`, `ac2 session open` and `ac2 selftest duplex` all
  carry its text, which names FlexASIO for devices without their own driver.

## 3b. Timers and scheduling on Windows

Windows ends a timed wait on its system clock tick, 15.6 ms unless a process asks for a
finer one, except a high-resolution waitable timer (Windows 10 1803 and later). Checked in
the sources (Rust 1.95 std, tokio 1.53, mio 1.2, winit 0.30.13):

- std `thread::sleep` uses a high-resolution waitable timer, so it is on time.
- std's timed waits (`Condvar::wait_timeout`, `mpsc::recv_timeout`, `park_timeout`) are
  `WaitOnAddress` with a millisecond timeout, so they end on a tick.
- tokio's timer wheel has 1 ms slots, but the runtime parks in mio's
  `GetQueuedCompletionStatusEx` with a millisecond timeout, so its timers end on a tick too.
- winit's `WaitUntil` uses a high-resolution waitable timer.
- libzmq's `zmq_poll` on Windows is `select` with a millisecond timeout, so it ends on a
  tick.

| Place | Waits with | On a 15.6 ms tick | Verdict | Fix |
|---|---|---|---|---|
| Fan-out hand-off (`ac2d/src/fanout.rs`): drains the capture ring, meters, feeds jobs | `timer::Sleeper` (condvar timed wait), every `1/fps`, 33 ms | 33 → 33–47 ms: 30 fps arrives as an uneven 21–30 | hurts the display | `timer::fine_resolution()` (1 ms) for the daemon's life |
| Same thread, under load | normal priority | Windows lifts a starved thread only after about 4 s; the ring holds about 2 s | can lose audio | MMCSS "Pro Audio" at thread start (`timer::pro_audio_thread()`); the ASIO callback itself is the driver's thread |
| I/O thread publish gap (`ac2d/src/io.rs`) | `zmq_poll`, held-back frame `period/2` ≈ 16 ms | 16 → up to 31 ms | latency, uneven frames | fine resolution |
| Job emit retry, math operand wait (`ac2d/src/jobs`) | `recv_timeout`, ≥ one period | + up to a tick | harmless; the hand-off normally wakes them | fine resolution covers them |
| Leq / SPL per second (`jobs/spl`) | sample counts, not clocks | none | fine | — |
| Control loop: leases, recovery, autosave (`control.rs`, `autosave.rs`) | `recv_timeout`, 100 ms to seconds | + up to a tick | fine | — |
| Outbox retry (`ac2d/src/outbox.rs`), preview, detect, stream stop | `thread::sleep` 1–2 ms | high-resolution timer | fine | — |
| Replay and fake devices (`ac2-audio`) | `Sleeper`, `thread::sleep` 1 ms | bursts of a tick's worth | fine (lossless, or test-only) | fine resolution covers it |
| UI link display clock (`ac2-ui/src/conn/mod.rs`) | tokio `sleep_until`, 33 ms | 33–47 ms between fetches | hurts the display | `timeBeginPeriod(1)` in `ac2-ui` `main` for the app's life, also when the daemon is elsewhere |
| Client data wait (`ac2-client`, Windows has no fd for the runtime) | tokio sleep 8 ms poll | 8 → 16 ms: each frame up to a tick late | latency | same |
| UI repaint (`request_repaint_after`), present | winit `WaitUntil`, vsync | high-resolution timer | fine | — |
| CLI `watch`, lease refresh, sync tick | tokio, 100 ms or more | + up to a tick | fine | — |
| ASIO host thread, open handshake (`asio_host/open.rs`) | `park_timeout` 100 ms, `sleep` 2 ms | + up to a tick on the park | fine (open time only) | — |

`timeBeginPeriod` is process-wide and counted: the daemon (`ac2d::Handle`) and the app
each hold one, and an embedded daemon in the app holds both. Windows 11 may ignore it for a
process whose windows are all minimised or hidden. Then the app's own fetches slow down
while nobody looks, and the capture still drains, since the ring holds 2 s and MMCSS
covers the drain thread. MMCSS registration happens when the fan-out thread starts, never in
the audio callback. A refused registration (the scheduler service stopped) is logged, and
the thread runs at normal priority.

## 4. Testing without Windows hardware, in this order

**Audio safety.** Every stage below runs on the dummy server, a virtual device or a test
signal path. No automated run passes `--emit`, and nothing here touches a real interface's
outputs.

**(0) Linux unit tests** run in the quick tier against a scripted in-process driver, with
the same intent as `fake`. They cover:
- sample-position jumps → exact `DISCONTINUITY`;
- a repeated buffer index;
- a reset during start;
- each sample type's conversion against refgen vectors;
- message → event mapping;
- the `AtomicPtr` lifecycle across stop/start.

**(a) Wine + wineasio in Docker, on Linux.** This runs the real ASIO code path: COM
loading, `ASIOCreateBuffers`, `bufferSwitchTimeInfo`, messages.
- Setup: wine64 and wineasio 1.3.0 (LGPL-2.1, `make 64`, `wineasio-register`; its build no
  longer needs the SDK headers, and it loads `libjack.so.0` at run time) against
  `jackd -d dummy -r 48000 -p 256`. ac2 is cross-built for `x86_64-pc-windows-gnu`.
- Runs:
  - `ac2 selftest duplex --backend asio` for 10 min.
  - `jack_connect` from the client's output port to its input port: the loopback delay
    must be identical across 20 stop/start cycles. That shows the shared index is exact.
  - Kill and restart `jackd` → `not_delivering`, then reopen.
  - `jack_bufsize` → wineasio's reset request, then reopen.
- This is also the binding spike of §1.
- Limits: wineasio is one well-behaved driver. Wine is not Windows (no MMCSS, no real
  scheduling). There are no vendor driver quirks such as double callbacks, spurious resets
  or Int24 packing. It says nothing about latency or real-time headroom.
- It runs locally. It is not part of the push tier, and not yet a CI job.

**Result (`tools/wine-asio/ac2.sh`, 48 kHz, period 256).** Planned runs as they turned out:
- `ac2 selftest duplex --backend asio` passes for 30 s: index `exact`, clock
  `single_callback`, Float32 both ways, no xrun, no break in the index.
- 20 opens of WineASIO in one process, each a new driver load and origin, with
  `ac2:out_1 → ac2:in_1` connected in JACK: every impulse (580) came back exactly 256 samples
  (one period) later. The shared index is exact across restarts.
- `jack_bufsize` 256 → 512 mid-stream: wineasio's reset request ends the stream, and the
  next open comes up at 512 frames, index exact, with no break.
- WineASIO's channel names come through (`in_1`, `out_1`, …); it reports latency 0/0 and no
  overload reporting (noted in `enumerate`).
- Not run: the daemon. `ac2` gets no reply from `ac2d` under Wine 10 (libzmq over loopback
  TCP stays idle; the `select` poller makes no difference), so the 30 s daemon session and
  the daemon's own reopen are left to Windows CI (§4b). Killing `jackd` is not run either.
- Found and fixed: azo's driver creation released a reference before `init`, and
  wineasio's `Release` reads unset state then, so the second load in one process
  double-freed (the azo fork now creates with the CLSID as the IID, §1).

**(b) GitHub `windows-latest`, all-OS dispatch only.**
- Setup: a virtual audio endpoint, then FlexASIO (MIT; ASIO over PortAudio WASAPI)
  installed silently.
  - Scream (Ms-PL) gives only an output endpoint.
  - VB-CABLE, which `LABSN/sound-ci-helpers` installs on runners, also gives a render →
    capture loopback. That lets the loopback correlation run.
  - Hosted runners need `audiosrv` started.
- What it proves: the MSVC build with the chosen binding, driver enumeration from the real
  registry, loading under real COM, a duplex selftest, and a clean stop.
- As built (`tools/release/windows-asio.ps1`, run by `ci.yml` on an all-OS dispatch; it is
  the Windows audio pass): CI first checks that `ac2 selftest duplex --backend asio`
  without a driver fails with the named FlexASIO message, then installs VB-CABLE and
  FlexASIO and grants desktop apps microphone access (FlexASIO opens the cable through
  WASAPI, which refuses capture otherwise). The script writes
  `%USERPROFILE%\FlexASIO.toml` (`backend = "Windows WASAPI"`, input and output `device`
  set to VB-CABLE's endpoint names as Windows lists them, found with `Get-PnpDevice`),
  lists the ASIO devices with channel names, rates and buffer sizes, runs a silent 20 s
  `ac2 selftest duplex --backend asio` (logging rate, buffer, index exactness, clock and
  formats, so FlexASIO's sample-position behaviour is on record), and records a silent 30 s
  daemon session (no stop, no discontinuity, ≥ 90 % of the frames). Not yet run: it lands
  untested on Windows.
- App: after the script, `ac2-ui --embedded --backend asio --smoke` hosts its daemon on
  ASIO, waits for the device list and quits. The step passes when the app exits 0 within
  120 s, prints `smoke: ok`, and lists `FlexASIO` with inputs and outputs. It opens no
  session, so nothing plays.
- Soak (dispatch only): `gh workflow run ci.yml --ref <branch> -f soak_minutes=60` runs the
  Windows job and keeps the recorded session open that long. Each minute goes into
  `soak-samples.csv` in the `virtual-audio-logs` artifact: recorded frames against
  rate × time, discontinuities, the stopped flag, and the daemon's CPU, working set, private
  bytes and handles. It passes with no discontinuity, frames within 0.1 % of rate × time and
  the daemon alive. Private-byte growth over 64 MB is a warning, not a failure, because
  history legitimately grows; a leak shows as steady growth in the CSV. Xruns are counted
  from the recording's sidecar at the end. This is the closest a hosted runner comes to the
  hardware gate (d). Its clock is a virtual cable through WASAPI, with no interface, no
  vendor driver and no UI load.
- Limits: FlexASIO is WASAPI shared mode underneath, so this shows plumbing, not
  pro-driver timing.

**(c) VM with USB passthrough of the Babyface Pro.** A last resort, used only when (a) and
(b) pass and a fault shows up only on RME's driver. Outputs stay muted or disconnected.

**(d) Hardware gate.** A Windows tester with a real interface runs the existing 1 h duplex
gate (`ac2 selftest duplex` procedure, 0 discontinuities, a stable loopback delay) on the
ASIO build. It also changes the buffer size in the driver's control panel mid-run, and the
run must reopen by itself. Until this passes, ASIO is shipped as preview.

## 5. Packaging and CI

- No SDK and no libclang are needed. The ASIO backend is part of every Windows build, so
  the `windows` release job's plain `cargo build --release` is the ASIO build.
- Artifact: the one `ac2-<ver>-windows-x64.msi` (and `.zip`), MIT.
- `ci.yml`: the Windows clippy, build and tests compile the ASIO backend with everything
  else (the driver module included); on an all-OS dispatch the FlexASIO pass of §4b is the
  Windows audio pass. A Linux job for §4a is not added: it needs the 7 GB image.

## 6. Open questions

1. ~~Is a weeks-old, single-author crate acceptable as a dependency?~~ Decided: yes,
   through the fork (above), with each fork change offered upstream; upstream answered and
   released a fix within a day. Still open: vendor drivers on real Windows, unchecked so far
   (only wineasio and FlexASIO).
2. ~~Trademark: plain-text "ASIO" in the UI and docs?~~ Decided (W3): yes. Naming the
   interface a device speaks is nominative use; the SDK's logo and wording guidelines bind
   SDK licensees, and ac2 uses no SDK. No logo, and the docs carry the attribution line
   "ASIO is a trademark of Steinberg Media Technologies GmbH".
3. ~~Should ASIO go into the default MSI with WASAPI kept as the fallback?~~ Decided (W1):
   ASIO only, in the one MSI; FlexASIO for devices without a driver.
4. Which channel subset do drivers accept in `ASIOCreateBuffers` (a sparse `input_map`)?
   Settled for ac2's side: the backend asks for exactly the mapped inputs (deduplicated)
   and maps them back correctly (scripted-driver test; under wineasio inputs 3 and 7 with
   `out_1 → in_7` put the impulses on the second block channel only, `ac2.sh`). Still
   open: a vendor driver that wants contiguous channels; it would refuse at
   `createBuffers` with its own words.
5. ~~Two MSIs and their UpgradeCode~~: moot, there is one MSI.
6. ~~Clock source: read-only in Settings, or the driver panel?~~ Decided: read-only (the
   list in Settings and `ac2 devices`, the current one in the session), set in the
   driver's panel (§3a). ~~What happens when the clock changes?~~ Decided (operator): any
   clock event (reset, resync, buffer-size or rate change, a rate of 0) stops emitting at
   once and says so in plain words; nothing resumes by itself (§3a, *Clock events cut the
   output*). Still to see on hardware: what a driver does when the external clock it
   follows goes away (a reset, a rate of 0, an error, or silence; silence is caught by the
   not-delivering bound, the output then fades over the usual 20 ms at the teardown).
7. ~~`kAsioGetInternalBufferSamples` (ASIO 2.3): fold into the stated latency?~~ Decided
   (operator: it shows added latency from the device's own DSP, and not every driver
   implements it): asked at every open and shown beside the stated latency, `not
   reported` when absent (§3a, *Driver buffering*). Per the SDK it is already part of
   `getLatencies`, so it is not added a second time; a vendor driver that reports it
   outside its latencies would show on the loopback delay, which is the measurement.
8. ~~Can a capture-only session on ASIO coexist with a WASAPI output device?~~ Moot: there
   is no WASAPI backend.
