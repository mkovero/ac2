//! The IOProc: create, start, stop, destroy, and the trampoline the HAL calls each cycle.
//!
//! The state the trampoline reaches (closure, indexer, channel tables) is boxed before
//! `AudioDeviceStart` and freed only after `AudioDeviceStop` and
//! `AudioDeviceDestroyIOProcID` have returned, after which the HAL never calls it. The
//! trampoline allocates nothing, takes no lock and makes no system call.

use std::ffi::c_void;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use objc2_core_audio as ca;
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp, AudioTimeStampFlags};

use crate::HostClock;
use crate::device::Device;
use crate::index::Indexer;
use crate::layout::{ChannelTable, Cycle, Input, Output};
use crate::listener::Listener;
use crate::types::{
    CycleTime, DeviceProperty, Error, FourCc, Gap, IoConfig, IoStats, Scope, StreamFormat,
    StreamLayout, StreamSet,
};

/// Most buffers (streams, or channels of non-interleaved streams) per direction. The
/// trampoline builds its buffer views in fixed arrays of this size on the stack.
pub const MAX_BUFFERS: usize = 64;

#[derive(Debug, Default)]
struct Counters {
    cycles: AtomicU64,
    overloads: AtomicU64,
    gaps: AtomicU64,
    frames_lost: AtomicU64,
    mismatched: AtomicU64,
    panicked: AtomicBool,
}

struct State<F> {
    f: F,
    config: IoConfig,
    indexer: Indexer,
    input: ChannelTable,
    output: ChannelTable,
    clock: HostClock,
    /// Bumped by the overload listener, taken by the next cycle.
    overloads: Arc<AtomicU32>,
    counters: Arc<Counters>,
    dead: bool,
}

/// A running IOProc. Dropping it stops it; once [`IoProc::stop`] (or drop) returns, the
/// closure is never called again.
pub struct IoProc {
    device: Device,
    id: ca::AudioDeviceIOProcID,
    state: Option<NonNull<c_void>>,
    free: unsafe fn(NonNull<c_void>),
    counters: Arc<Counters>,
    _overload: Option<Listener>,
}

// SAFETY: `state` is only touched by the HAL's I/O thread while running and freed here
// after the IOProc is destroyed; the closure inside is `Send`.
unsafe impl Send for IoProc {}
// SAFETY: `&IoProc` only reads the atomic counters; the state is never reached through it.
unsafe impl Sync for IoProc {}

impl fmt::Debug for IoProc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IoProc")
            .field("device", &self.device)
            .field("stats", &self.stats())
            .finish()
    }
}

fn status(call: &'static str, st: i32) -> Result<(), Error> {
    if st == 0 {
        Ok(())
    } else {
        Err(Error::Status {
            call,
            selector: FourCc(0),
            status: st,
        })
    }
}

fn table(device: Device, dir: Scope, used: bool) -> Result<ChannelTable, Error> {
    if !used {
        return Ok(ChannelTable::default());
    }
    let layout: StreamLayout = device.layout(dir)?;
    if layout
        .streams
        .iter()
        .any(|s| s.format != StreamFormat::Float32)
    {
        return Err(Error::Unsupported(
            "a stream whose virtual format is not 32-bit float",
        ));
    }
    let t = ChannelTable::new(&layout);
    if t.buffers().len() > MAX_BUFFERS {
        return Err(Error::Unsupported("more than 64 buffers in one direction"));
    }
    Ok(t)
}

pub(crate) fn start<F>(device: Device, config: IoConfig, f: F) -> Result<IoProc, Error>
where
    F: FnMut(Cycle<'_, '_>) + Send + 'static,
{
    let input = table(device, Scope::Input, config.input)?;
    let output = table(device, Scope::Output, config.output)?;
    let overloads = Arc::new(AtomicU32::new(0));
    let counters = Arc::new(Counters::default());
    let overload = {
        let o = Arc::clone(&overloads);
        device
            .listen(&[DeviceProperty::ProcessorOverload], move |_| {
                o.fetch_add(1, Ordering::Relaxed);
            })
            .ok()
    };
    let state = Box::new(State {
        f,
        config,
        indexer: Indexer::new(),
        input,
        output,
        clock: HostClock::host(),
        overloads,
        counters: Arc::clone(&counters),
        dead: false,
    });
    let state = NonNull::from(Box::leak(state)).cast::<c_void>();
    let mut io = IoProc {
        device,
        id: None,
        state: Some(state),
        free: free::<F>,
        counters,
        _overload: overload,
    };
    let mut id: ca::AudioDeviceIOProcID = None;
    // SAFETY: the trampoline matches the state's type; the state lives until `IoProc`'s
    // drop has destroyed the IOProc.
    let st = unsafe {
        ca::AudioDeviceCreateIOProcID(
            device.object_id(),
            Some(trampoline::<F>),
            state.as_ptr(),
            NonNull::from(&mut id),
        )
    };
    status("AudioDeviceCreateIOProcID", st)?;
    io.id = id;
    if config.input {
        use_input_streams(device, id, config.input_streams);
    }
    // SAFETY: a proc id just created on this device.
    let st = unsafe { ca::AudioDeviceStart(device.object_id(), io.id) };
    status("AudioDeviceStart", st)?;
    Ok(io)
}

/// Turns off the input streams `used` leaves out, so the HAL does not convert them. Best
/// effort: a device that refuses keeps every stream on, which costs conversion time and
/// changes no sample the closure reads.
fn use_input_streams(device: Device, id: ca::AudioDeviceIOProcID, used: StreamSet) {
    let Some(proc) = id.and_then(|f| NonNull::new(f as *mut c_void)) else {
        return;
    };
    let Ok(layout) = device.layout(Scope::Input) else {
        return;
    };
    let count = layout.streams.len();
    if used.covers(count) || used.is_empty() {
        return;
    }
    let _ = crate::property::set_stream_usage(
        device.object_id(),
        crate::property::scope(Scope::Input),
        proc,
        count,
        used,
    );
}

unsafe fn free<F>(p: NonNull<c_void>) {
    // SAFETY: `p` came from `Box::leak` of a `State<F>` in `start`, and the IOProc that
    // used it is destroyed.
    drop(unsafe { Box::from_raw(p.cast::<State<F>>().as_ptr()) });
}

impl IoProc {
    /// Counters so far.
    pub fn stats(&self) -> IoStats {
        let c = &self.counters;
        IoStats {
            cycles: c.cycles.load(Ordering::Acquire),
            overloads: c.overloads.load(Ordering::Relaxed),
            gaps: c.gaps.load(Ordering::Relaxed),
            frames_lost: c.frames_lost.load(Ordering::Relaxed),
            mismatched: c.mismatched.load(Ordering::Relaxed),
            panicked: c.panicked.load(Ordering::Acquire),
        }
    }

    /// Waits until the closure has been called once, or fails with
    /// [`Error::StartTimedOut`].
    pub fn wait_first_cycle(&self, timeout: Duration) -> Result<(), Error> {
        let deadline = Instant::now() + timeout;
        while self.counters.cycles.load(Ordering::Acquire) == 0 {
            if Instant::now() >= deadline {
                return Err(Error::StartTimedOut);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }

    /// The device it runs on.
    pub fn device(&self) -> Device {
        self.device
    }

    /// Stops and destroys the IOProc.
    pub fn stop(mut self) -> Result<(), Error> {
        self.teardown()
    }

    fn teardown(&mut self) -> Result<(), Error> {
        let mut result = Ok(());
        if self.id.is_some() {
            // SAFETY: the proc id created in `start` on this device.
            let st = unsafe { ca::AudioDeviceStop(self.device.object_id(), self.id) };
            result = status("AudioDeviceStop", st);
            // SAFETY: as above. Destroying a proc the HAL may still call is the HAL's to
            // refuse; whether the state may be freed is decided below.
            let st = unsafe { ca::AudioDeviceDestroyIOProcID(self.device.object_id(), self.id) };
            if result.is_ok() {
                result = status("AudioDeviceDestroyIOProcID", st);
            }
            self.id = None;
        }
        match self.state.take() {
            Some(p) if result.is_ok() => {
                // SAFETY: the IOProc that used the state is stopped and destroyed (or was
                // never created), so the HAL never calls the trampoline with it again.
                unsafe { (self.free)(p) };
            }
            // A failed stop or destroy: the HAL may still call the trampoline with this
            // pointer, so the state is leaked on purpose. A leaked box beats a
            // use-after-free.
            _ => {}
        }
        result
    }
}

impl Drop for IoProc {
    fn drop(&mut self) {
        let _ = self.teardown();
    }
}

fn host_ns(clock: &HostClock, t: &AudioTimeStamp) -> Option<u64> {
    t.mFlags
        .contains(AudioTimeStampFlags::HostTimeValid)
        .then(|| clock.ns(t.mHostTime))
}

fn sample_time(t: &AudioTimeStamp) -> Option<f64> {
    t.mFlags
        .contains(AudioTimeStampFlags::SampleTimeValid)
        .then_some(t.mSampleTime)
}

/// The buffers of a list as raw parts, checked against the table: `None` when the list
/// does not have the table's buffers and channels.
///
/// # Safety
///
/// `list` must point to a valid `AudioBufferList` for the call.
unsafe fn buffers(
    list: NonNull<AudioBufferList>,
    table: &ChannelTable,
    parts: &mut [(*mut f32, usize); MAX_BUFFERS],
) -> Option<usize> {
    // SAFETY: valid for the call, per the contract.
    let n = unsafe { list.as_ref() }.mNumberBuffers as usize;
    let want = table.buffers();
    if n != want.len() {
        return None;
    }
    // SAFETY: field projection on a valid pointer, no read.
    let first = unsafe { std::ptr::addr_of!((*list.as_ptr()).mBuffers) }.cast::<AudioBuffer>();
    let mut frames = None;
    for (i, &channels) in want.iter().enumerate() {
        // SAFETY: the list has `n` buffers.
        let b = unsafe { &*first.add(i) };
        if b.mData.is_null() {
            // A stream the HAL does not serve (turned off by stream usage) reads as
            // silence, whatever channel count its empty buffer states.
            parts[i] = (std::ptr::null_mut(), 0);
            continue;
        }
        if b.mNumberChannels != channels {
            return None;
        }
        let samples = b.mDataByteSize as usize / std::mem::size_of::<f32>();
        let f = samples / channels.max(1) as usize;
        if *frames.get_or_insert(f) != f {
            return None;
        }
        parts[i] = (b.mData.cast::<f32>(), samples);
    }
    Some(frames.unwrap_or(0))
}

/// Zeroes every buffer of a list, whatever its shape: the output of a cycle that is not
/// rendered must be silence, not what the HAL's buffer held before.
///
/// # Safety
///
/// `list` must point to a valid `AudioBufferList` for the call, its buffers writable.
unsafe fn zero_list(list: NonNull<AudioBufferList>) {
    // SAFETY: valid for the call, per the contract.
    let n = unsafe { list.as_ref() }.mNumberBuffers as usize;
    // SAFETY: field projection on a valid pointer, no read.
    let first = unsafe { std::ptr::addr_of!((*list.as_ptr()).mBuffers) }.cast::<AudioBuffer>();
    for i in 0..n {
        // SAFETY: the list has `n` buffers.
        let b = unsafe { &*first.add(i) };
        if !b.mData.is_null() {
            // SAFETY: the HAL's buffer of `mDataByteSize` bytes, writable for the call.
            unsafe { std::ptr::write_bytes(b.mData.cast::<u8>(), 0, b.mDataByteSize as usize) };
        }
    }
}

unsafe extern "C-unwind" fn trampoline<F>(
    _device: u32,
    now: NonNull<AudioTimeStamp>,
    in_data: NonNull<AudioBufferList>,
    in_time: NonNull<AudioTimeStamp>,
    out_data: NonNull<AudioBufferList>,
    out_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32
where
    F: FnMut(Cycle<'_, '_>) + Send + 'static,
{
    // SAFETY: `client` is the `State<F>` registered with this trampoline, alive until the
    // IOProc is destroyed; the HAL calls one IOProc on one thread at a time.
    let st = unsafe { &mut *client.cast::<State<F>>() };
    // SAFETY: the HAL's timestamps are valid for the call.
    let (now, in_time, out_time) = unsafe { (now.as_ref(), in_time.as_ref(), out_time.as_ref()) };

    let mut in_parts = [(std::ptr::null_mut::<f32>(), 0usize); MAX_BUFFERS];
    let mut out_parts = [(std::ptr::null_mut::<f32>(), 0usize); MAX_BUFFERS];
    let in_frames = if st.config.input {
        // SAFETY: the HAL's buffer lists are valid for the call.
        unsafe { buffers(in_data, &st.input, &mut in_parts) }
    } else {
        Some(0)
    };
    let out_frames = if st.config.output {
        // SAFETY: as above.
        unsafe { buffers(out_data, &st.output, &mut out_parts) }
    } else {
        Some(0)
    };
    if out_frames.is_none() || !st.config.output {
        // A list that does not match the table is never handed to the closure, so nothing
        // else zeroes it: silence, never stale samples past the renderer's gating.
        // SAFETY: the HAL's output list, valid and writable for the call.
        unsafe { zero_list(out_data) };
    }
    let n_in = st.input.buffers().len();
    let n_out = st.output.buffers().len();

    // Output views first: they are zeroed whatever happens next.
    let mut out_bufs: [&mut [f32]; MAX_BUFFERS] = std::array::from_fn(|_| &mut [][..]);
    for (slot, &(p, len)) in out_bufs.iter_mut().zip(&out_parts[..n_out]) {
        if !p.is_null() {
            // SAFETY: the HAL's output buffer of `len` samples, valid and exclusively ours
            // for the call.
            *slot = unsafe { std::slice::from_raw_parts_mut(p, len) };
        }
    }
    for b in &mut out_bufs[..n_out] {
        b.fill(0.0);
    }

    let frames = match (st.config.input, st.config.output, in_frames, out_frames) {
        (_, _, None, _) | (_, _, _, None) => None,
        (true, true, Some(i), Some(o)) if i != o && i != 0 && o != 0 => None,
        (_, _, Some(i), Some(o)) => Some(i.max(o)),
    };
    let Some(frames) = frames else {
        st.counters.mismatched.fetch_add(1, Ordering::Relaxed);
        return 0;
    };
    if st.dead || frames == 0 {
        return 0;
    }

    let mut in_bufs: [&[f32]; MAX_BUFFERS] = [&[]; MAX_BUFFERS];
    for (slot, &(p, len)) in in_bufs.iter_mut().zip(&in_parts[..n_in]) {
        if !p.is_null() {
            // SAFETY: the HAL's input buffer of `len` samples, valid for the call.
            *slot = unsafe { std::slice::from_raw_parts(p, len) };
        }
    }

    let indexed = if st.config.input { in_time } else { out_time };
    let (index, gap) = st.indexer.advance(sample_time(indexed), frames as u32);
    let overloads = st.overloads.swap(0, Ordering::Relaxed);
    let c = &st.counters;
    if overloads > 0 {
        c.overloads
            .fetch_add(u64::from(overloads), Ordering::Relaxed);
    }
    match gap {
        Gap::First | Gap::None => {}
        Gap::Forward(n) => {
            c.gaps.fetch_add(1, Ordering::Relaxed);
            c.frames_lost.fetch_add(n, Ordering::Relaxed);
        }
        Gap::Backward => {
            c.gaps.fetch_add(1, Ordering::Relaxed);
        }
    }
    let time = CycleTime {
        index,
        frames: frames as u32,
        gap,
        estimated: st.indexer.estimated(),
        overloads,
        input_sample_time: sample_time(in_time),
        output_sample_time: sample_time(out_time),
        now_ns: st.clock.ns(now.mHostTime),
        input_ns: host_ns(&st.clock, in_time),
        output_ns: host_ns(&st.clock, out_time),
        rate_scalar: if indexed
            .mFlags
            .contains(AudioTimeStampFlags::RateScalarValid)
        {
            indexed.mRateScalar
        } else {
            1.0
        },
    };
    let cycle = Cycle {
        input: Input::new(&in_bufs[..n_in], &st.input, frames),
        output: Output::new(&mut out_bufs[..n_out], &st.output, frames),
        time,
    };
    let f = &mut st.f;
    // The default panic hook writes the first panic's message to stderr from this thread:
    // accepted, it happens once (the proc is dead after it) and catch_unwind still holds.
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| f(cycle))) {
        // Freeing the payload would deallocate on the I/O thread; it happens once.
        std::mem::forget(payload);
        st.dead = true;
        c.panicked.store(true, Ordering::Release);
        for b in &mut out_bufs[..n_out] {
            b.fill(0.0);
        }
    }
    c.cycles.fetch_add(1, Ordering::Release);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{StreamFormat, StreamInfo, StreamLayout};

    #[test]
    fn a_list_that_does_not_match_the_table_is_refused_and_zeroed() {
        // The table expects one mono stream; the HAL hands a stereo one.
        let table = ChannelTable::new(&StreamLayout {
            streams: vec![StreamInfo {
                channels: 1,
                interleaved: true,
                format: StreamFormat::Float32,
            }],
        });
        let mut data = vec![0.5f32; 2 * 8];
        let mut list = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [AudioBuffer {
                mNumberChannels: 2,
                mDataByteSize: (data.len() * std::mem::size_of::<f32>()) as u32,
                mData: data.as_mut_ptr().cast(),
            }],
        };
        let ptr = NonNull::from(&mut list);
        let mut parts = [(std::ptr::null_mut::<f32>(), 0usize); MAX_BUFFERS];
        // SAFETY: `list` and `data` outlive both calls.
        assert_eq!(unsafe { buffers(ptr, &table, &mut parts) }, None);
        // SAFETY: as above.
        unsafe { zero_list(ptr) };
        assert!(data.iter().all(|v| *v == 0.0));
    }
}
