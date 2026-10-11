//! What runs inside the driver's callbacks: the per-stream state they reach through one
//! static slot, the `bufferSwitch` body, and the answers to the driver's messages.
//!
//! ASIO callbacks carry no user pointer, so the running stream is reached through a
//! [`Slot`]: an atomic pointer set before `start()` and cleared only once `stop()` has
//! returned and no callback is still inside. Nothing here allocates, locks or makes a
//! system call; the driver calls in a switch (`getSamplePosition` for a driver without time
//! info, `outputReady`) are the ones the ASIO host sample makes from its callback too.

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering};

use super::format::Format;
use super::position::PositionClock;
use crate::block::{BlockProducer, BlockStamp};
use crate::events::{BackendEvents, EventLatch};
use crate::output::{OutputRenderer, OutputStamp};

/// The calls a switch makes into the driver.
pub(crate) trait RtDriver: Send + Sync {
    /// `getSamplePosition`: frames since `start()`, or `None` when the driver has none.
    fn sample_position(&self) -> Option<i64>;
    /// `outputReady`; false when the driver does not implement it.
    fn output_ready(&self) -> bool;
}

/// What `bufferSwitchTimeInfo` hands over, already unpacked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimeInfo {
    /// `samplePosition`, when flagged valid.
    pub(crate) position: Option<i64>,
    /// `systemTime` in ns, when flagged valid.
    pub(crate) system_ns: Option<u64>,
}

/// One channel's double buffer and its format.
#[derive(Debug)]
pub(crate) struct Channel {
    pub(crate) bufs: [*mut u8; 2],
    pub(crate) format: Format,
}

/// Lifecycle of a stream as the callbacks see it.
const STARTING: u8 = 0;
const RUNNING: u8 = 1;
const STOPPING: u8 = 2;

/// Everything the callback mutates. Only the holder of [`Engine::busy`] touches it.
pub(crate) struct Rt {
    pub(crate) inputs: Vec<Channel>,
    pub(crate) outputs: Vec<Channel>,
    pub(crate) frames: u32,
    pub(crate) producer: BlockProducer,
    pub(crate) renderer: OutputRenderer,
    pub(crate) clock: PositionClock,
    pub(crate) latch: EventLatch,
    pub(crate) driver: Box<dyn RtDriver>,
    pub(crate) output_ready: bool,
    last: Option<(i32, Option<i64>)>,
}

impl Rt {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        inputs: Vec<Channel>,
        outputs: Vec<Channel>,
        frames: u32,
        producer: BlockProducer,
        renderer: OutputRenderer,
        rate: u32,
        events: Arc<BackendEvents>,
        driver: Box<dyn RtDriver>,
    ) -> Self {
        Self {
            inputs,
            outputs,
            frames,
            producer,
            renderer,
            clock: PositionClock::new(rate),
            latch: EventLatch::new(events),
            driver,
            output_ready: true,
            last: None,
        }
    }
}

/// One open stream as its callbacks reach it.
pub(crate) struct Engine {
    pub(crate) events: Arc<BackendEvents>,
    rate: f64,
    phase: AtomicU8,
    /// Switches that produced a block.
    indexed: AtomicU64,
    /// Switches seen twice with the same buffer half and position, skipped.
    duplicates: AtomicU64,
    /// Switches that arrived while another was still running, skipped.
    overlaps: AtomicU64,
    estimated: AtomicBool,
    latency_dirty: AtomicBool,
    busy: AtomicBool,
    rt: UnsafeCell<Rt>,
}

// SAFETY: `rt` holds raw pointers into the driver's buffers and is only touched by the one
// thread holding `busy`; everything else is atomics. The driver's buffers live until
// `disposeBuffers`, which the stream guard calls only after the slot is cleared.
unsafe impl Send for Engine {}
unsafe impl Sync for Engine {}

impl Engine {
    pub(crate) fn new(rt: Rt, rate: f64, events: Arc<BackendEvents>) -> Self {
        Self {
            events,
            rate,
            phase: AtomicU8::new(STARTING),
            indexed: AtomicU64::new(0),
            duplicates: AtomicU64::new(0),
            overlaps: AtomicU64::new(0),
            estimated: AtomicBool::new(false),
            latency_dirty: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            rt: UnsafeCell::new(rt),
        }
    }

    /// `start()` returned: switches from now on are indexed.
    pub(crate) fn started(&self) {
        self.phase.store(RUNNING, Ordering::Release);
    }

    /// The stream is being stopped: driver requests no longer end it.
    pub(crate) fn stopping(&self) {
        self.phase.store(STOPPING, Ordering::Release);
    }

    /// Switches that produced a block so far.
    pub(crate) fn indexed(&self) -> u64 {
        self.indexed.load(Ordering::Acquire)
    }

    /// Whether the index fell back to a running count.
    pub(crate) fn estimated(&self) -> bool {
        self.estimated.load(Ordering::Relaxed)
    }

    /// Repeated switches skipped so far.
    #[cfg(test)]
    pub(crate) fn duplicates(&self) -> u64 {
        self.duplicates.load(Ordering::Relaxed)
    }

    /// Takes the "latencies changed" note the driver left.
    pub(crate) fn take_latency_change(&self) -> bool {
        self.latency_dirty.swap(false, Ordering::AcqRel)
    }

    /// One `bufferSwitch` / `bufferSwitchTimeInfo`. `half` is the buffer half the driver
    /// hands over, `time` the time info when the driver gave it, `callback_ns` the host
    /// clock now.
    pub(crate) fn switch(&self, half: i32, time: Option<TimeInfo>, callback_ns: u64) {
        if self.busy.swap(true, Ordering::Acquire) {
            // A second switch while one runs: the driver lost the period. Never touch the
            // state twice at once.
            self.overlaps.fetch_add(1, Ordering::Relaxed);
            self.events.xrun();
            return;
        }
        // SAFETY: `busy` was false and is now ours, so no other reference to `rt` exists.
        let rt = unsafe { &mut *self.rt.get() };
        self.switch_inner(rt, half, time, callback_ns);
        self.busy.store(false, Ordering::Release);
    }

    fn switch_inner(&self, rt: &mut Rt, half: i32, time: Option<TimeInfo>, callback_ns: u64) {
        let h = (half & 1) as usize;
        let frames = rt.frames as usize;
        if self.phase.load(Ordering::Acquire) != RUNNING {
            // The switches a driver makes inside `start()` prime the output, on the
            // caller's thread, before the stream runs: they get silence and no index, so
            // index 0 is the first switch both directions share after the start.
            for o in &rt.outputs {
                // SAFETY: each half of a created buffer holds `frames` samples.
                unsafe { o.format.silence(o.bufs[h], frames) };
            }
            return;
        }
        let (position, system_ns) = match time {
            Some(t) => (
                t.position.or_else(|| rt.driver.sample_position()),
                t.system_ns,
            ),
            None => (rt.driver.sample_position(), None),
        };
        // Some USB drivers call twice per period with the same half: the second call
        // carries no new audio.
        if let Some((last_half, last_pos)) = rt.last
            && last_half == half
            && (position.is_none() || position == last_pos)
        {
            self.duplicates.fetch_add(1, Ordering::Relaxed);
            return;
        }
        rt.last = Some((half, position));
        let (start, mut flags) = rt.clock.advance(position, rt.frames, system_ns);
        if rt.clock.estimated() {
            self.estimated.store(true, Ordering::Relaxed);
        }
        flags |= rt.latch.take();
        let inputs = &rt.inputs;
        rt.producer.push_with(
            BlockStamp {
                start_sample: start,
                frames: rt.frames,
                flags,
                callback_ns,
                // The driver's system time is on another clock and, on some drivers, in
                // milliseconds: the index carries the timing, not this.
                capture_ns: None,
            },
            |f, c| {
                let ch = &inputs[c];
                // SAFETY: `f < frames`, and each half holds `frames` samples.
                unsafe { ch.format.read(ch.bufs[h], f) }
            },
        );
        let outputs = &rt.outputs;
        rt.renderer.render(
            OutputStamp {
                start_sample: start,
                frames: rt.frames,
                flags,
                callback_ns,
                playback_ns: None,
            },
            outputs.len(),
            |f, c, v| {
                if let Some(ch) = outputs.get(c) {
                    // SAFETY: as above.
                    unsafe { ch.format.write(ch.bufs[h], f, v) };
                }
            },
        );
        if rt.output_ready && !rt.driver.output_ready() {
            rt.output_ready = false;
        }
        self.indexed.fetch_add(1, Ordering::Release);
    }

    /// Whether a driver request should end the stream now: not while it starts (drivers
    /// send spurious requests from inside `start()`), and not before the first indexed
    /// switch.
    fn live(&self) -> bool {
        self.phase.load(Ordering::Acquire) == RUNNING && self.indexed() > 0
    }

    /// `sampleRateDidChange`.
    pub(crate) fn rate_changed(&self, hz: f64) {
        // 0 means the driver lost its clock; any other rate than the stream's means the
        // index no longer counts the frames it was opened at.
        if hz != self.rate && self.live() {
            self.events.end();
        }
    }
}

/// ASIO message selectors ac2 answers (`asioMessage`).
pub(crate) mod selector {
    pub(crate) const SELECTOR_SUPPORTED: i32 = 1;
    pub(crate) const ENGINE_VERSION: i32 = 2;
    pub(crate) const RESET_REQUEST: i32 = 3;
    pub(crate) const BUFFER_SIZE_CHANGE: i32 = 4;
    pub(crate) const RESYNC_REQUEST: i32 = 5;
    pub(crate) const LATENCIES_CHANGED: i32 = 6;
    pub(crate) const SUPPORTS_TIME_INFO: i32 = 7;
    pub(crate) const SUPPORTS_TIME_CODE: i32 = 8;
    pub(crate) const OVERLOAD: i32 = 15;
}

/// The answer to `asioMessage(selector, value)`, and its effect on the running stream.
///
/// - Reset and resync requests end the stream: the recovery reopens the same configuration
///   from scratch, which is what a reset asks the host to do.
/// - A buffer size change is refused (answer 0, and not listed as supported): a driver must
///   then ask for a reset rather than change the size under a callback whose buffers and
///   index step were sized at open. A driver that announces one anyway also ends the
///   stream.
/// - An overload is an xrun on the next block.
/// - Changed latencies are noted for the control side to re-read off the callback.
pub(crate) fn answer(engine: Option<&Engine>, selector: i32, value: i32) -> i32 {
    use selector::*;
    match selector {
        SELECTOR_SUPPORTED => i32::from(matches!(
            value,
            SELECTOR_SUPPORTED
                | ENGINE_VERSION
                | RESET_REQUEST
                | RESYNC_REQUEST
                | LATENCIES_CHANGED
                | SUPPORTS_TIME_INFO
                | OVERLOAD
        )),
        ENGINE_VERSION => 2,
        SUPPORTS_TIME_INFO => 1,
        SUPPORTS_TIME_CODE => 0,
        RESET_REQUEST | RESYNC_REQUEST | BUFFER_SIZE_CHANGE => {
            if let Some(e) = engine
                && e.live()
            {
                e.events.end();
            }
            i32::from(selector != BUFFER_SIZE_CHANGE)
        }
        LATENCIES_CHANGED => {
            if let Some(e) = engine {
                e.latency_dirty.store(true, Ordering::Release);
            }
            1
        }
        OVERLOAD => {
            if let Some(e) = engine {
                e.events.xrun();
            }
            0
        }
        _ => 0,
    }
}

/// Where the callbacks find the running stream.
pub(crate) struct Slot {
    engine: AtomicPtr<Engine>,
    /// Callbacks currently inside, so clearing can wait for them to leave.
    users: AtomicUsize,
}

impl Slot {
    /// An empty slot.
    pub(crate) const fn new() -> Self {
        Self {
            engine: AtomicPtr::new(std::ptr::null_mut()),
            users: AtomicUsize::new(0),
        }
    }

    /// Points the callbacks at `engine`, which must stay alive until [`Self::clear`]
    /// returns.
    pub(crate) fn install(&self, engine: &Engine) {
        self.engine
            .store(std::ptr::from_ref(engine).cast_mut(), Ordering::SeqCst);
    }

    /// Detaches the engine and waits for every callback still using it to leave. Called
    /// after the driver's `stop()` has returned, so the wait is at most one switch.
    pub(crate) fn clear(&self) {
        self.engine.store(std::ptr::null_mut(), Ordering::SeqCst);
        while self.users.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
    }

    /// True while an engine is installed.
    #[cfg(test)]
    pub(crate) fn is_set(&self) -> bool {
        !self.engine.load(Ordering::SeqCst).is_null()
    }

    /// Runs `f` with the installed engine, if any.
    #[inline]
    pub(crate) fn with<R>(&self, f: impl FnOnce(Option<&Engine>) -> R) -> R {
        self.users.fetch_add(1, Ordering::SeqCst);
        let p = self.engine.load(Ordering::SeqCst);
        // SAFETY: `clear` nulls the pointer before it waits for `users` to reach zero, and
        // the engine outlives `clear`; a non-null pointer read while counted as a user is
        // therefore alive for the whole call.
        let r = f(unsafe { p.as_ref() });
        self.users.fetch_sub(1, Ordering::SeqCst);
        r
    }
}
