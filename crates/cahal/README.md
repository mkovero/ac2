# cahal

A small Rust binding to the Core Audio HAL on macOS: devices, their properties and property
listeners, and **one duplex IOProc per device whose input and output share an exact sample
index**.

The HAL calls an IOProc once per cycle with the input buffers it read and the output buffers
to fill, both stamped on the device's one sample-time line (`mSampleTime`). `cahal` turns
those stamps into one index per cycle: the input and output of a cycle take the same index,
and a jump in the sample time is the exact number of frames the device lost. That is the
model JACK and ASIO give a host, which AUHAL-based bindings (two units, two callbacks) do not.

```rust,no_run
use cahal::{IoConfig, Scope, System};

let dev = System::default_device(Scope::Input)?.expect("a default input");
let info = dev.info()?;
println!("{} ({}), {} Hz, {} in", info.name, info.uid, dev.nominal_rate()?, info.input.channels());
let io = dev.start_io(IoConfig { input: true, output: true }, |mut cycle| {
    // The HAL's real-time I/O thread: no allocation, locks or system calls here.
    let t = cycle.time;
    let input = cycle.input.channel(0);
    let mut out = cycle.output.channel_mut(0);
    for i in 0..t.frames as usize {
        out.set(i, 0.0 * input.get(i)); // outputs start zeroed; this one stays silent
    }
})?;
io.wait_first_cycle(std::time::Duration::from_secs(2))?;
println!("{:?}", io.stats());
io.stop()?;
# Ok::<(), cahal::Error>(())
```

## What it covers

- `System`: the device list, the default devices, lookup by UID, listeners (device list,
  defaults, coreaudiod restarts).
- `Device`: UID, name, transport, stream layout and virtual formats, channel names, nominal
  rate (set and wait), available rates, buffer size and range, latencies and safety offsets
  per direction, clock domain, clock sources, hog mode, property listeners.
- `Device::start_io`: one IOProc serving the directions asked for. Each cycle hands the
  closure a `Cycle`: per-channel views of the (possibly several, possibly interleaved) stream
  buffers, and a `CycleTime` with the shared index, the gap from the previous cycle
  (`Gap::Forward(n)` is exactly `n` frames lost), overloads since the last cycle and the
  host times of both sides. Output buffers are zeroed before the closure runs; a panicking
  closure is never called again and its outputs stay silent.
- `consent` (default feature): the microphone authorization status and a bounded request.
  Without a grant, macOS blocks a capture IOProc's start on a consent prompt instead of
  failing it, so check first.

Every stream used must have a 32-bit float virtual format (the HAL's own conversion; true of
every current device seen). The closure owns its real-time discipline; `cahal`'s side of
the call allocates nothing, locks nothing and makes no system call.

## Other platforms

The HAL code is `cfg(target_os = "macos")`. Elsewhere the crate is its pure part — the data
types, the `Indexer` and the `ChannelTable` with the cycle views — with no dependencies and
no `unsafe`, so code built on it can be tested on any host.

## Safety

All `unsafe` is in four files: `property.rs` (typed property reads and writes, sizes
checked), `ioproc.rs` (create/start/stop/destroy and the trampoline), `listener.rs`
(register, unregister before free) and `consent.rs` (two AVFoundation calls). Each block
carries a `// SAFETY:` comment (`clippy::undocumented_unsafe_blocks` is denied).

## Status

Developed inside [ac2](https://github.com/mkovero/ac2) until its API settles, then moved
to a repository of its own. MIT licensed.
