# wine-asio: real ASIO code paths on Linux

Runs Windows ASIO host code on Linux, with no Windows machine: binaries cross-compiled for
`x86_64-pc-windows-msvc` (cargo-xwin) run under Wine and load
[wineasio](https://github.com/wineasio/wineasio), an ASIO driver that is a JACK client.
JACK runs the dummy driver, so no sound device is involved.

Two probes (`probe/`, a standalone Cargo project, not an ac2 workspace member):

- `azo_probe` — [`azo`](https://crates.io/crates/azo) 0.4.0: ASIO over COM directly, no
  Steinberg SDK. Loads WineASIO, prints channels/buffer sizes/latencies/clock sources,
  creates buffers for 2 in + 2 out, runs, and records per bufferSwitch: buffer index,
  ASIOTime sample position / system time, `getSamplePosition`, thread id, wall time.
  `PROBE_IMPULSE=1` writes a -20 dBFS single-sample impulse on output 1 every 32 buffers
  and finds it on input 1, for a loopback-delay check.
- `cpal_probe` — cpal 0.18 `asio` feature (asio-sys + Steinberg SDK): opens one input and
  one output stream, writes silence, records per callback direction, frames, cpal
  timestamps and `ASIOGetSamplePosition` (called straight into the SDK library).

Both only store integers into a preallocated table inside the callback; printing is after.

```
tools/wine-asio/run.sh             # build image (~30 min first time), run all, print summaries
tools/wine-asio/run.sh --no-build  # reuse the image
RATE=44100 PERIOD=128 tools/wine-asio/run.sh --no-build
```

`run.sh` writes `tools/wine-asio/out/`: `build.log` (both probes for msvc; `azo_probe` again
with no cpal/asio-sys and no `CPAL_ASIO_DIR`, and for `x86_64-pc-windows-gnu`), `cpal.log`,
`azo.log` (silence, two stop/start cycles), `azo-plain.log` (time info refused, so plain
`bufferSwitch`), `azo-loop-{1,2,3}.log` (three separate processes, `probe:out_1 →
probe:in_1` connected in JACK), `jackd.log`. Cargo cache: `$CACHE_DIR/target` (default
`/work/ac2-scratch/wine-asio/target`). The container runs with `--memory 8g --cpus 4`
and `--ulimit memlock=-1` (JACK clients mlock their shared memory; without it wineasio
cannot open its client); cargo uses `CARGO_BUILD_JOBS=4`.

## ac2's backend: `ac2.sh`

`tools/wine-asio/ac2.sh` (needs the image from `run.sh`) cross-builds ac2d and ac2 (ASIO
is in every Windows build), plus `ac2-loop/` (a standalone check on ac2-audio, never shipped), for
`x86_64-pc-windows-msvc`, and runs them on WineASIO with JACK's dummy driver
(`WINEASIO_CLIENT_NAME=ac2`):

1. `ac2 selftest duplex --backend asio` for `SECONDS_REC` (30) s, silent, in process:
   block continuity, xruns, both clocks.
2. `ac2-loop`: the device list (WineASIO's channel names), then `CYCLES` (20) opens of
   WineASIO through `AsioBackend`, each a new driver load and stream origin, with a
   -20 dBFS single-sample impulse every 4800 samples on output 1; the script keeps
   `ac2:out_1 → ac2:in_1` connected inside JACK. Every impulse's delay on the stream's
   shared index must be the same in every cycle. `ac2-loop` refuses any driver but
   WineASIO.
3. `ac2-loop MODE=bufsize` (silent): `jack_bufsize` to twice the period mid-stream;
   wineasio asks for a reset, the stream must end, and a new open must come up at the new
   size with an unbroken index — the backend's half of the daemon's recovery.
4. A sparse input map, with WineASIO set to 8 inputs for it: `ac2 selftest duplex --in
   3,7` (silent), then `ac2-loop MODE=sparse` with `ac2:out_1 → ac2:in_7`: the impulses
   must arrive on the block's second channel only.
5. `ac2-loop MODE=panel` (silent): the driver's control panel with no stream and over a
   running one; prints what the driver answered and whether the stream kept running.
   wineasio answers at once (it vforks `wineasio-settings`); the process then ends with
   SIGKILL at exit (status 137), with or without that program installed, so the check is
   the `PASS` line.

The daemon is built but not run. Under Wine 10, `ac2` gets no reply from `ac2d` over
loopback TCP: both sit idle in libzmq (the daemon's io thread in `zmq_poll`, its reactor in
wepoll), with the TCP connection established. Building libzmq with the `select` poller for
both did not change it. This is ZeroMQ under Wine, not ASIO; the daemon session on ASIO is
covered by Windows CI on FlexASIO (`tools/release/windows-asio.ps1`).

The build uses CI's pinned prebuilt MSVC libsodium (`tools/release/fetch-libsodium.sh`);
`sodium-make` stands in for `make` in libsodium's source tree, because
libsodium-sys-stable builds from source first and that cannot target MSVC from Linux.

Logs in `out/ac2/`. Caches: `$CACHE_DIR/ac2-target` (cargo target), `$CACHE_DIR/registry`
and `$CACHE_DIR/git` (cargo downloads; the build fetches crates, so it needs the network).

## Image

- Debian trixie (digest-pinned), Wine 10.0 (64-bit prefix), JACK2 1.9.22 + example tools,
  LLVM 19, MinGW-w64 GCC 14, jq.
- wineasio 1.3.0 from source (`make 64`; Debian keeps Wine's headers in
  `/usr/include/wine/wine/windows`, passed as `WINEBUILD_INCLUDEDIR`), `wineasio64.dll`
  registered with `regsvr32`. Registry: 2 in / 2 out, no connection to JACK system ports.
- Steinberg ASIO SDK 2.3.4, downloaded from Steinberg at image build with a pinned SHA-256
  into `/opt/asiosdk` (`CPAL_ASIO_DIR`). Never committed: it is dual-licensed (proprietary
  Steinberg ASIO license or GPLv3), and neither allows putting it into this MIT repository.
  Only `cpal_probe` needs it.
- Rust 1.95 + cargo-xwin 0.23.1. cargo-xwin downloads and unpacks the MSVC CRT and Windows
  SDK once (~25 min, most of the build time). asio-sys's build script runs bindgen (libclang)
  and compiles the SDK's C++ with clang-cl; it runs `vcvarsall.bat` unless `VCINSTALLDIR` is
  set, so the image sets it to a dummy path. azo's build script needs `rustfmt`.

## Findings (48 kHz, period 256)

- **azo builds without any SDK**, for `-msvc` and `-gnu`; its dependency tree has no
  asio-sys and no C/C++.
- **azo position clock is exact**: every bufferSwitch advances `samplePosition` by exactly
  the buffer size (1876 of 1876 steps over two stop/start cycles; 3 × 750 in the loopback
  runs). `ASIOTime.samplePosition` equals `getSamplePosition` called in the callback; the
  buffer index alternates every switch. Each `start()` restarts the position at 0, and the
  first switch (position 0) runs on the thread calling `start()` (the driver primes the
  output), the rest on the JACK thread.
- **Loopback delay** out_1 → in_1 inside JACK: exactly 256 samples (one period) for all 23
  impulses in each of 3 separate process runs.
- **cpal**: input and output callbacks always come from the same bufferSwitch on the same
  thread, input first, strictly alternating, equal frames, identical callback instants.
  cpal drops the priming switch (its first callback is at position 256). cpal does not
  expose the sample position; its instants come from the driver's system time.
- **System time is coarse under wineasio**: it is `timeGetTime()` in ms, so steps are 2–6 ms
  against a nominal 5.333 ms period. Only the sample position can give input and output one
  exact shared sample index; wall-clock timestamps cannot.
- wineasio sends `kAsioLatenciesChanged` when JACK connections change, and reports input
  and output latency 0.

## Limits — Wine is not Windows

- COM, the registry and threading are Wine's. The callback thread is a JACK thread wrapped
  by Wine, not a vendor driver thread; MMCSS is a stub, so scheduling says nothing about
  Windows.
- wineasio is one driver with JACK timing. Vendor drivers' sample-position and system-time
  quality, buffer-size choices, `kAsioResetRequest` handling, latency reports and clock
  sources must still be checked on real hardware.
- The JACK dummy driver paces with the system clock; jitter here is the container's
  scheduler, not an audio clock.
