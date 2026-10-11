# ASIO backend (Windows)

**Status: design only, no code.** On Windows, ac2 runs on cpal WASAPI in shared mode: two
`IAudioClient`s on two threads, a same-clock relation that the API cannot prove, and render
xruns that are never reported (`spike-audio-duplex.md` §1). ASIO is the Windows path where
one driver callback (`bufferSwitch`) serves capture and playback on one clock, like JACK.
The operator rates it a must-have. PLAN.md §12, decision 1, leaves its licence open; §1
below settles it.

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
1. ac2's source stays MIT. The `asio` cargo feature stays off in default builds, and the
   default MSI stays WASAPI-only and MIT. Both choices from §12 hold.
2. The binding is chosen by a short spike against wineasio (§4a). The spike checks
   enumeration, buffer creation, `bufferSwitchTimeInfo` with a valid sample position,
   messages, and a clean stop/restart. ac2 keeps one binding, never two.
   - **azo passes:** the ASIO build links no Steinberg code and stays MIT. Shipping
     it in the default MSI then becomes a plain product decision.
   - **azo fails:** ac2 uses its own thin binding over the SDK (bindgen + `cc`, the
     `asio-sys` recipe, over a pinned and hashed SDK, never downloaded unchecked). The
     binaries built with `asio` then ship as a **separate GPLv3 variant**: `LICENSE` plus
     `COPYING`, and the corresponding source (the ac2 tag, the SDK zip, the build recipe)
     attached to the release. MIT code may be combined into a GPLv3 work. The dependency
     licences still have to be checked for GPLv3 compatibility in that job (libzmq is
     MPL-2.0, which is compatible).
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
- Feature `asio`, Windows only, in `crates/ac2-audio/src/asio_host/`.
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

**Selection.** The CLI and the app offer `asio` as a backend only in builds with the
feature
(`BackendArg::Asio`). The tester guide (`testing/windows/README.md`) gains a section once
there is a build to test.

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
- It runs locally and as a Linux CI job on dispatch. It is not part of the push tier.

**(b) GitHub `windows-latest`, all-OS dispatch only.**
- Setup: a virtual audio endpoint, then FlexASIO (MIT; ASIO over PortAudio WASAPI)
  installed silently.
  - Scream (Ms-PL) gives only an output endpoint.
  - VB-CABLE, which `LABSN/sound-ci-helpers` installs on runners, also gives a render →
    capture loopback. That lets the loopback correlation run.
  - Hosted runners need `audiosrv` started.
- What it proves: the MSVC build with the chosen binding, driver enumeration from the real
  registry, loading under real COM, a 2 min duplex selftest, and a clean stop.
- Limits: FlexASIO is WASAPI shared mode underneath, so this shows plumbing, not
  pro-driver timing.

**(c) VM with USB passthrough of the Babyface Pro.** A last resort, used only when (a) and
(b) pass and a fault shows up only on RME's driver. Outputs stay muted or disconnected.

**(d) Hardware gate.** A Windows tester with a real interface runs the existing 1 h duplex
gate (`ac2 selftest duplex` procedure, 0 discontinuities, a stable loopback delay) on the
ASIO build. It also changes the buffer size in the driver's control panel mid-run, and the
run must reopen by itself. Until this passes, ASIO is shipped as preview.

## 5. Packaging and CI

**If azo is chosen.**
- No SDK and no libclang are needed. `cargo build --features asio` runs in the existing
  `windows` release job as a second build.
- Artifact: `ac2-<ver>-windows-x64-asio.msi` (and `.zip`), MIT.

**If the SDK binding is chosen.**
- The release job downloads the pinned SDK URL, checks the SHA-256 above (a mismatch fails
  the job, so a changed SDK is reviewed before any binary links it), unzips it to `CPAL_ASIO_DIR`, and
  uses the runner's LLVM for bindgen.
- Artifact: `ac2-<ver>-windows-x64-asio-gpl3.msi` (and `.zip`), carrying GPLv3 `COPYING`
  in place of the MIT `LICENSE` in `packaging/windows/ac2.wxs`. The release also attaches
  `ac2-<ver>-asio-source.zip` (the tag, the SDK zip and the build steps).
- The dependency licence check (for example `cargo about` or `cargo deny`) runs in that job.

**Both cases.**
- The default MSI is unchanged.
- The two MSIs install the same binaries to the same place, so they must not both be
  installed (open question 5).
- `ci.yml` gains a Windows `--features asio` clippy and build under `all_os=true`, and a
  dispatch-only Linux job for §4a.

## 6. Open questions

1. Does azo hold up in the §4a spike (time info, messages, stop/restart, thread affinity)?
   Is a weeks-old, single-author crate acceptable as a dependency, or should ac2 vendor or
   fork it?
2. Trademark: with azo there is no SDK licence at all. Is plain-text "ASIO" in the UI and
   the docs nominative use with no logo duty? The SDK guidelines bind SDK licensees.
3. If both bindings pass, should ASIO go into the default MSI (one build), with WASAPI kept
   as the fallback backend?
4. Which channel subset do drivers accept in `ASIOCreateBuffers` (a sparse `input_map`)?
   Some drivers may require contiguous channels.
5. Two MSIs: should they share an UpgradeCode so that one replaces the other, or keep
   separate product codes with a launch condition that refuses to install the second?
6. Clock source (`ASIOGetClockSources` / `ASIOSetClockSource`): show it read-only in
   Settings, or leave it to the driver panel? An external-word-clock loss is another reset
   path to test.
7. `kAsioGetInternalBufferSamples` (ASIO 2.3) reports buffering inside the driver. Should
   it be folded into the stated latency?
8. Can a capture-only session on ASIO coexist with a WASAPI output device? The answer is
   probably no: it is the same "one clock" rule as multi-device (`multi-device.md`).
