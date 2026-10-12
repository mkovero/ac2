//! The ASIO backend against a scripted in-process driver: negotiation, the shared index,
//! repeated switches, messages and the slot's lifecycle, with no Windows and no COM.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::backend::{AsioHost, DriverSource};
use super::engine::{RtDriver, Slot, TimeInfo, answer, selector};
use super::format::Format;
use super::open::{
    AsioCode, AsioDriver, BufferSizes, ChannelDesc, ClockDesc, DriverError, device_caps,
    one_device, open_stream,
};
use crate::backend::{
    Backend, ClockSource, DeviceId, DeviceSelector, DriverBuffering, DuplexRequest, IndexExactness,
    StaticLatency,
};
use crate::block::{BlockFlags, BlockHeader};
use crate::error::{AudioError, Unavailability, Unsupported};
use crate::events::ResetReason;
use crate::generator::{GeneratorHandle, Sine, generator};
use crate::level::MaxLevel;
use crate::output::{OutputSource, OutputState};
use crate::stream::DuplexStream;

const F32: i32 = 19;
const I24: i32 = 17;
const FRAMES: u32 = 256;

/// How the scripted driver reports its position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Timing {
    /// `bufferSwitchTimeInfo` with a valid position.
    TimeInfo,
    /// Plain `bufferSwitch`; the position comes from `getSamplePosition`.
    Plain,
    /// Neither: no position at all.
    None,
}

struct Script {
    inputs: Vec<ChannelDesc>,
    outputs: Vec<ChannelDesc>,
    sizes: BufferSizes,
    rates: Vec<f64>,
    timing: Timing,
    /// Messages the driver sends from inside `start()`.
    messages_in_start: Vec<i32>,
    /// `getClockSources`; `None` answers `ASE_NotPresent`.
    clocks: Option<Vec<ClockDesc>>,
}

impl Default for Script {
    fn default() -> Self {
        let ch = |n: &str| ChannelDesc {
            name: n.into(),
            sample_type: F32,
        };
        Self {
            inputs: vec![ch("Analog 1"), ch("Analog 2"), ch("ADAT 1"), ch("ADAT 2")],
            outputs: vec![ch("Main L"), ch("Main R")],
            sizes: BufferSizes {
                min: 64,
                max: 2048,
                preferred: FRAMES as i32,
                granularity: -1,
            },
            rates: vec![44_100.0, 48_000.0],
            timing: Timing::TimeInfo,
            messages_in_start: Vec::new(),
            clocks: None,
        }
    }
}

/// Buffers the driver handed out: per channel, both halves back to back.
#[derive(Default)]
struct Buffers {
    channels: Vec<(bool, u16, Format)>,
    memory: Vec<Vec<u8>>,
    frames: usize,
}

/// What the scripted control panel does when opened: set the driver to a rate and a
/// preferred buffer size, then ask the host for a reset.
type PanelChange = (f64, i32);

struct Scripted {
    script: Script,
    slot: &'static Slot,
    /// The buffer sizes the driver offers now (its panel may change the preferred one).
    sizes: Mutex<BufferSizes>,
    /// The interface is plugged in.
    present: AtomicBool,
    /// `init` refuses with this message (empty: refuses without a word).
    init_refusal: Mutex<Option<String>>,
    /// `createBuffers` fails with this code.
    create_refusal: Mutex<Option<AsioCode>>,
    /// What opening the panel changes.
    panel: Mutex<Option<PanelChange>>,
    panel_calls: AtomicUsize,
    rate: Mutex<f64>,
    latencies: Mutex<(u32, u32)>,
    /// `kAsioGetInternalBufferSamples`; `None` answers `ASE_NotPresent`.
    internal: Mutex<Option<(u32, u32)>>,
    buffers: Mutex<Buffers>,
    position: Arc<AtomicI64>,
    half: AtomicUsize,
    calls: Mutex<Vec<&'static str>>,
    output_ready_calls: Arc<AtomicUsize>,
    running: AtomicBool,
}

struct ScriptedRt {
    position: Arc<AtomicI64>,
    has_position: bool,
    output_ready_calls: Arc<AtomicUsize>,
}

impl RtDriver for ScriptedRt {
    fn sample_position(&self) -> Option<i64> {
        self.has_position
            .then(|| self.position.load(Ordering::SeqCst))
    }
    fn output_ready(&self) -> bool {
        self.output_ready_calls.fetch_add(1, Ordering::SeqCst);
        true
    }
}

impl Scripted {
    fn new(script: Script) -> Arc<Self> {
        Self::with_slot(script, Box::leak(Box::new(Slot::new())))
    }

    fn with_slot(script: Script, slot: &'static Slot) -> Arc<Self> {
        Arc::new(Self {
            sizes: Mutex::new(script.sizes),
            script,
            slot,
            present: AtomicBool::new(true),
            init_refusal: Mutex::new(None),
            create_refusal: Mutex::new(None),
            panel: Mutex::new(None),
            panel_calls: AtomicUsize::new(0),
            rate: Mutex::new(48_000.0),
            latencies: Mutex::new((32, 48)),
            internal: Mutex::new(None),
            buffers: Mutex::new(Buffers::default()),
            position: Arc::new(AtomicI64::new(0)),
            half: AtomicUsize::new(0),
            calls: Mutex::new(Vec::new()),
            output_ready_calls: Arc::new(AtomicUsize::new(0)),
            running: AtomicBool::new(false),
        })
    }

    fn log(&self, call: &'static str) {
        self.calls.lock().expect("scripted state").push(call);
    }

    /// Input sample of device input `ch` at absolute driver position `pos`.
    fn signal(ch: u16, pos: i64) -> f32 {
        (f32::from(ch) + 1.0) * 0.1 + (pos % 1000) as f32 * 1e-4
    }

    /// One period: fills the inputs for the current position and calls the switch.
    fn switch_at(&self, pos: i64, half: usize) {
        self.position.store(pos, Ordering::SeqCst);
        {
            let mut b = self.buffers.lock().expect("scripted state");
            let frames = b.frames;
            let channels = b.channels.clone();
            for (i, (input, ch, format)) in channels.into_iter().enumerate() {
                let base = b.memory[i].as_mut_ptr();
                // SAFETY: half `half` of channel `i` holds `frames` samples.
                let p = unsafe { base.add(half * frames * format.bytes()) };
                for f in 0..frames {
                    // SAFETY: as above.
                    unsafe {
                        if input {
                            format.write(p, f, Self::signal(ch, pos + f as i64));
                        } else {
                            // Garbage the switch must overwrite.
                            format.write(p, f, 0.77);
                        }
                    }
                }
            }
        }
        let time = match self.script.timing {
            Timing::TimeInfo => Some(TimeInfo {
                position: Some(pos),
                system_ns: None,
            }),
            Timing::Plain | Timing::None => None,
        };
        self.slot
            .with(|e| e.map(|e| e.switch(half as i32, time, pos as u64 * 1000)));
    }

    /// The next regular period.
    fn tick(&self) {
        let half = self.half.fetch_xor(1, Ordering::SeqCst) ^ 1;
        let pos = self.position.load(Ordering::SeqCst) + i64::from(FRAMES);
        self.switch_at(pos, half);
    }

    fn message(&self, sel: i32, value: i32) -> i32 {
        self.slot.with(|e| answer(e, sel, value))
    }

    /// Output sample `frame` of output `ch` in buffer half `half`.
    fn output(&self, ch: u16, half: usize, frame: usize) -> f32 {
        let b = self.buffers.lock().expect("scripted state");
        let i = b
            .channels
            .iter()
            .position(|&(input, c, _)| !input && c == ch)
            .expect("output buffer");
        let format = b.channels[i].2;
        // SAFETY: inside the channel's buffer.
        unsafe {
            format.read(
                b.memory[i].as_ptr().add(half * b.frames * format.bytes()),
                frame,
            )
        }
    }
}

impl Scripted {
    /// `ASE_NotPresent` while unplugged, as drivers answer once their hardware is gone.
    fn plugged(&self) -> Result<(), DriverError> {
        if self.present.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(DriverError {
                code: Some(AsioCode::NotPresent),
                message: String::new(),
            })
        }
    }
}

impl AsioDriver for Scripted {
    fn init(&self) -> Result<(), DriverError> {
        self.log("init");
        if !self.present.load(Ordering::SeqCst) {
            return Err(DriverError::text("No ASIO device found"));
        }
        match self.init_refusal.lock().expect("scripted state").clone() {
            Some(m) => Err(DriverError::text(m)),
            None => Ok(()),
        }
    }
    fn clock_sources(&self) -> Result<Vec<ClockDesc>, DriverError> {
        self.script.clocks.clone().ok_or(DriverError {
            code: Some(AsioCode::NotPresent),
            message: String::new(),
        })
    }
    fn control_panel(&self) -> Result<(), DriverError> {
        self.log("control_panel");
        self.panel_calls.fetch_add(1, Ordering::SeqCst);
        if let Some((rate, preferred)) = self.panel.lock().expect("scripted state").take() {
            *self.rate.lock().expect("scripted state") = rate;
            self.sizes.lock().expect("scripted state").preferred = preferred;
            // As drivers do once their settings changed under a running host.
            self.message(selector::RESET_REQUEST, 0);
        }
        Ok(())
    }
    fn channel_counts(&self) -> Result<(u16, u16), DriverError> {
        self.plugged()?;
        Ok((
            self.script.inputs.len() as u16,
            self.script.outputs.len() as u16,
        ))
    }
    fn channel(&self, input: bool, index: u16) -> Result<ChannelDesc, DriverError> {
        let list = if input {
            &self.script.inputs
        } else {
            &self.script.outputs
        };
        list.get(usize::from(index))
            .cloned()
            .ok_or_else(|| DriverError::text("no such channel"))
    }
    fn buffer_sizes(&self) -> Result<BufferSizes, DriverError> {
        Ok(*self.sizes.lock().expect("scripted state"))
    }
    fn can_sample_rate(&self, hz: f64) -> bool {
        self.script.rates.contains(&hz)
    }
    fn sample_rate(&self) -> Result<f64, DriverError> {
        Ok(*self.rate.lock().expect("scripted state"))
    }
    fn set_sample_rate(&self, hz: f64) -> Result<(), DriverError> {
        self.log("set_sample_rate");
        *self.rate.lock().expect("scripted state") = hz;
        Ok(())
    }
    fn latencies(&self) -> Result<(u32, u32), DriverError> {
        Ok(*self.latencies.lock().expect("scripted state"))
    }
    fn can_report_overload(&self) -> bool {
        false
    }
    fn internal_buffering(&self) -> Option<(u32, u32)> {
        *self.internal.lock().expect("scripted state")
    }
    fn create_buffers(
        &self,
        channels: &[(bool, u16)],
        frames: u32,
    ) -> Result<Vec<[*mut u8; 2]>, DriverError> {
        self.log("create_buffers");
        if let Some(code) = *self.create_refusal.lock().expect("scripted state") {
            return Err(DriverError {
                code: Some(code),
                message: String::new(),
            });
        }
        let mut b = self.buffers.lock().expect("scripted state");
        b.frames = frames as usize;
        b.channels.clear();
        b.memory.clear();
        let mut out = Vec::new();
        for &(input, ch) in channels {
            let desc = self.channel(input, ch)?;
            let format = Format::from_asio(desc.sample_type).expect("scripted format");
            let mut mem = vec![0xEEu8; 2 * frames as usize * format.bytes()];
            let p = mem.as_mut_ptr();
            // SAFETY: the second half starts inside the allocation.
            out.push([p, unsafe { p.add(frames as usize * format.bytes()) }]);
            b.channels.push((input, ch, format));
            b.memory.push(mem);
        }
        Ok(out)
    }
    fn start(&self) -> Result<(), DriverError> {
        self.log("start");
        for &m in &self.script.messages_in_start {
            self.message(m, 0);
        }
        // The priming switch, on the caller's thread, at position 0.
        self.half.store(0, Ordering::SeqCst);
        self.switch_at(0, 0);
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn stop(&self) -> Result<(), DriverError> {
        self.log("stop");
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
    fn dispose_buffers(&self) -> Result<(), DriverError> {
        self.log("dispose_buffers");
        assert!(
            !self.slot.is_set(),
            "buffers disposed while callbacks can run"
        );
        Ok(())
    }
    fn rt(&self) -> Box<dyn RtDriver> {
        Box::new(ScriptedRt {
            position: Arc::clone(&self.position),
            has_position: self.script.timing == Timing::Plain,
            output_ready_calls: Arc::clone(&self.output_ready_calls),
        })
    }
}

fn level() -> MaxLevel {
    MaxLevel::from_peak_db(-20.0).expect("level")
}

fn open(drv: &Arc<Scripted>, req: DuplexRequest) -> Result<DuplexStream, AudioError> {
    open_wait(drv, req, Duration::ZERO)
}

fn open_wait(
    drv: &Arc<Scripted>,
    req: DuplexRequest,
    wait: Duration,
) -> Result<DuplexStream, AudioError> {
    let released = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&released);
    open_stream(
        Arc::clone(drv) as Arc<dyn AsioDriver>,
        drv.slot,
        DeviceId("Scripted ASIO".into()),
        req,
        wait,
        Box::new(move || r.store(true, Ordering::SeqCst)),
    )
}

fn blocks(s: &mut DuplexStream) -> Vec<(BlockHeader, Vec<f32>)> {
    let mut out = Vec::new();
    loop {
        let mut v = Vec::new();
        match s.capture().pop_into(&mut v) {
            Some(h) => out.push((h, v)),
            None => return out,
        }
    }
}

#[test]
fn caps_carry_the_drivers_channel_names_rates_and_sizes() {
    let drv = Scripted::new(Script::default());
    let c = device_caps(&*drv, "Scripted ASIO", true).expect("caps");
    let input = c.input.expect("input");
    assert_eq!(
        input.channel_names.as_deref(),
        Some(&["Analog 1", "Analog 2", "ADAT 1", "ADAT 2"].map(String::from)[..])
    );
    assert_eq!(input.max_channels, 4);
    assert_eq!(input.default_buffer, Some(256));
    assert_eq!(input.default_rate, Some(48_000));
    assert_eq!(
        input.rates.iter().map(|r| r.min).collect::<Vec<_>>(),
        [44_100, 48_000]
    );
    assert!(input.system_default);
    assert_eq!(
        c.latency,
        StaticLatency::Frames {
            input: 32,
            output: 48,
            driver_buffering: None,
        }
    );
    assert_eq!(c.duplex_clock, crate::ClockRelation::SingleCallback);
    assert!(c.notes.iter().any(|n| n.contains("powers of two")));
    assert!(
        c.notes
            .iter()
            .any(|n| n.contains("cannot report overloads"))
    );

    // Repeated or empty names cannot tell channels apart: numbered instead.
    let mut s = Script::default();
    s.inputs[1].name = "Analog 1".into();
    s.outputs[0].name = String::new();
    s.outputs[1].sample_type = 3;
    let c = device_caps(&*Scripted::new(s), "x", false).expect("caps");
    assert_eq!(c.input.expect("in").channel_names, None);
    assert_eq!(c.output.as_ref().expect("out").channel_names, None);
    assert!(
        c.notes.iter().any(|n| n.contains("Float32MSB")),
        "{:?}",
        c.notes
    );
}

#[test]
fn both_directions_of_a_switch_share_one_index_from_the_position() {
    let drv = Scripted::new(Script::default());
    let mut s = open(&drv, DuplexRequest::new(vec![0, 2], 2, level())).expect("open");
    // The priming switch inside start() wrote silence and indexed nothing.
    assert_eq!(drv.output(0, 0, 5), 0.0);
    assert!(blocks(&mut s).is_empty());
    for _ in 0..4 {
        drv.tick();
    }
    let b = blocks(&mut s);
    let starts: Vec<u64> = b.iter().map(|(h, _)| h.start_sample).collect();
    assert_eq!(starts, [0, 256, 512, 768]);
    assert_eq!(b[0].0.flags, BlockFlags::FIRST);
    assert!(b[1..].iter().all(|(h, _)| h.flags == BlockFlags::NONE));
    // Block channel 1 is device input 2, at the driver position of each frame.
    let (h, v) = &b[1];
    assert_eq!(h.channels, 2);
    assert_eq!(v[2 * 10], Scripted::signal(0, 512 + 10));
    assert_eq!(v[2 * 10 + 1], Scripted::signal(2, 512 + 10));
    let mut ticks = Vec::new();
    while let Some(t) = s.pop_output_tick() {
        ticks.push(t.start_sample);
    }
    assert_eq!(ticks, starts, "output ticks carry the same index");
    // Outputs were rendered (silence) over the garbage in both halves.
    assert_eq!(drv.output(1, 1, 100), 0.0);
    assert_eq!(drv.output(0, 0, 100), 0.0);
    assert!(drv.output_ready_calls.load(Ordering::SeqCst) >= 4);
    assert_eq!(s.negotiated().index, IndexExactness::Exact);
    assert_eq!(
        s.negotiated().latency,
        StaticLatency::Frames {
            input: 32,
            output: 48,
            driver_buffering: None,
        }
    );
}

#[test]
fn a_position_jump_is_an_exact_discontinuity() {
    let drv = Scripted::new(Script::default());
    let mut s = open(&drv, DuplexRequest::new(vec![0], 2, level())).expect("open");
    drv.tick();
    drv.tick();
    // The driver dropped three periods internally.
    let pos = drv.position.load(Ordering::SeqCst);
    drv.switch_at(pos + 4 * 256, 1);
    drv.tick();
    let b = blocks(&mut s);
    let got: Vec<(u64, BlockFlags)> = b.iter().map(|(h, _)| (h.start_sample, h.flags)).collect();
    assert_eq!(
        got,
        [
            (0, BlockFlags::FIRST),
            (256, BlockFlags::NONE),
            (512 + 3 * 256, BlockFlags::DISCONTINUITY),
            (512 + 4 * 256, BlockFlags::NONE),
        ]
    );
}

#[test]
fn a_repeated_switch_of_the_same_half_is_skipped() {
    let drv = Scripted::new(Script::default());
    let mut s = open(&drv, DuplexRequest::new(vec![0], 0, level())).expect("open");
    drv.tick();
    let pos = drv.position.load(Ordering::SeqCst);
    let half = drv.half.load(Ordering::SeqCst);
    drv.switch_at(pos, half);
    drv.tick();
    let starts: Vec<u64> = blocks(&mut s).iter().map(|(h, _)| h.start_sample).collect();
    assert_eq!(starts, [0, 256]);
    assert_eq!(drv.slot.with(|e| e.map_or(0, |e| e.duplicates())), 1);
}

#[test]
fn plain_buffer_switch_takes_the_position_from_the_driver() {
    let drv = Scripted::new(Script {
        timing: Timing::Plain,
        ..Script::default()
    });
    let mut s = open(&drv, DuplexRequest::new(vec![1], 1, level())).expect("open");
    drv.tick();
    let pos = drv.position.load(Ordering::SeqCst);
    drv.switch_at(pos + 2 * 256, 0);
    let got: Vec<(u64, BlockFlags)> = blocks(&mut s)
        .iter()
        .map(|(h, _)| (h.start_sample, h.flags))
        .collect();
    assert_eq!(
        got,
        [(0, BlockFlags::FIRST), (512, BlockFlags::DISCONTINUITY)]
    );
}

#[test]
fn a_driver_without_any_position_is_estimated() {
    let drv = Scripted::new(Script {
        timing: Timing::None,
        ..Script::default()
    });
    let ticker = {
        let drv = Arc::clone(&drv);
        std::thread::spawn(move || {
            // Switches before open() marks the stream started are priming and indexed by
            // nothing; tick until two are indexed.
            while drv.slot.with(|e| e.map_or(0, |e| e.indexed())) < 2 {
                drv.tick();
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    let mut s = open_wait(
        &drv,
        DuplexRequest::new(vec![0], 0, level()),
        Duration::from_secs(5),
    )
    .expect("open");
    ticker.join().expect("ticker");
    assert_eq!(s.negotiated().index, IndexExactness::Estimated);
    let b = blocks(&mut s);
    assert_eq!(b[0].0.flags, BlockFlags::FIRST | BlockFlags::GAP_ESTIMATED);
    assert_eq!(b[1].0.start_sample, 256);
}

#[test]
fn requests_during_start_are_dropped_and_later_ones_end_the_stream() {
    let drv = Scripted::new(Script {
        messages_in_start: vec![selector::RESET_REQUEST, selector::RESYNC_REQUEST],
        ..Script::default()
    });
    let s = open(&drv, DuplexRequest::new(vec![0], 0, level())).expect("open");
    assert!(
        !s.events().ended,
        "a request from inside start() is dropped"
    );
    // Before the first indexed switch it is dropped as well.
    assert_eq!(drv.message(selector::RESET_REQUEST, 0), 1);
    assert!(!s.events().ended);
    drv.tick();
    assert_eq!(drv.message(selector::RESET_REQUEST, 0), 1);
    assert!(s.events().ended);
}

#[test]
fn messages_map_to_events() {
    let drv = Scripted::new(Script::default());
    let mut s = open(&drv, DuplexRequest::new(vec![0], 0, level())).expect("open");
    drv.tick();
    // An overload is an xrun on the next block.
    drv.message(selector::OVERLOAD, 0);
    drv.tick();
    let b = blocks(&mut s);
    assert!(b[1].0.flags.contains(BlockFlags::XRUN), "{:?}", b[1].0);
    // A latency message with nothing changed is no configuration change.
    assert_eq!(drv.message(selector::LATENCIES_CHANGED, 0), 1);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(s.events().config_changes, 0);
    // A real change is.
    *drv.latencies.lock().expect("scripted state") = (64, 96);
    drv.message(selector::LATENCIES_CHANGED, 0);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while s.events().config_changes == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(s.events().config_changes, 1);
    drv.tick();
    assert!(
        blocks(&mut s)[0]
            .0
            .flags
            .contains(BlockFlags::CONFIG_CHANGE)
    );
    // A buffer size change is refused in place and ends the stream.
    assert_eq!(
        drv.message(selector::SELECTOR_SUPPORTED, selector::BUFFER_SIZE_CHANGE),
        0
    );
    assert!(!s.events().ended);
    assert_eq!(drv.message(selector::BUFFER_SIZE_CHANGE, 512), 0);
    assert!(s.events().ended);
}

#[test]
fn a_rate_change_ends_the_stream_and_static_answers_hold_without_one() {
    let drv = Scripted::new(Script::default());
    let s = open(&drv, DuplexRequest::new(vec![0], 0, level())).expect("open");
    drv.slot.with(|e| e.expect("engine").rate_changed(44_100.0));
    assert!(!s.events().ended, "dropped before the first indexed switch");
    drv.tick();
    drv.slot.with(|e| e.expect("engine").rate_changed(44_100.0));
    assert!(s.events().ended);
    assert_eq!(s.events().reset, Some(ResetReason::RateChange));
    for (sel, value, want) in [
        (
            selector::SELECTOR_SUPPORTED,
            selector::SUPPORTS_TIME_INFO,
            1,
        ),
        (selector::SELECTOR_SUPPORTED, selector::RESET_REQUEST, 1),
        (
            selector::SELECTOR_SUPPORTED,
            selector::SUPPORTS_TIME_CODE,
            0,
        ),
        (selector::ENGINE_VERSION, 0, 2),
        (selector::SUPPORTS_TIME_INFO, 0, 1),
        (selector::SUPPORTS_TIME_CODE, 0, 0),
        (selector::RESET_REQUEST, 0, 1),
        (99, 0, 0),
    ] {
        assert_eq!(
            answer(None, sel, value),
            want,
            "selector {sel} value {value}"
        );
    }
}

#[test]
fn the_slot_is_cleared_before_buffers_are_disposed_and_reused_after() {
    let drv = Scripted::new(Script::default());
    for round in 0..3 {
        let mut s = open(&drv, DuplexRequest::new(vec![0], 2, level())).expect("open");
        assert!(drv.slot.is_set());
        drv.position.store(0, Ordering::SeqCst);
        drv.tick();
        drv.tick();
        let b = blocks(&mut s);
        assert_eq!(b[0].0.flags, BlockFlags::FIRST, "round {round}");
        assert_eq!(b[0].0.start_sample, 0, "every start indexes from 0");
        drop(s);
        assert!(!drv.slot.is_set());
        // A switch after the stop finds no engine and does nothing.
        drv.tick();
    }
    let calls = drv.calls.lock().expect("scripted state").clone();
    assert_eq!(
        &calls[..4],
        ["create_buffers", "start", "stop", "dispose_buffers"]
    );
    assert_eq!(calls.len(), 12);
}

#[test]
fn buffers_only_for_the_channels_used_and_a_repeated_input_read_twice() {
    let drv = Scripted::new(Script::default());
    let mut s = open(&drv, DuplexRequest::new(vec![3, 1, 3], 1, level())).expect("open");
    let made: Vec<(bool, u16)> = drv
        .buffers
        .lock()
        .expect("scripted state")
        .channels
        .iter()
        .map(|&(i, c, _)| (i, c))
        .collect();
    assert_eq!(made, [(true, 1), (true, 3), (false, 0)]);
    drv.tick();
    let (h, v) = &blocks(&mut s)[0];
    assert_eq!(h.channels, 3);
    let pos = drv.position.load(Ordering::SeqCst);
    assert_eq!(v[0], Scripted::signal(3, pos));
    assert_eq!(v[1], Scripted::signal(1, pos));
    assert_eq!(v[2], Scripted::signal(3, pos));
}

#[test]
fn packed_24_bit_inputs_convert() {
    let mut script = Script::default();
    script.inputs[0].sample_type = I24;
    let drv = Scripted::new(script);
    let mut s = open(&drv, DuplexRequest::new(vec![0], 0, level())).expect("open");
    drv.tick();
    let (h, v) = &blocks(&mut s)[0];
    assert_eq!(s.negotiated().input_format, crate::SampleFormat::I24);
    let pos = drv.position.load(Ordering::SeqCst);
    for f in [0usize, 7, 255] {
        let want = Scripted::signal(0, pos + f as i64);
        assert!((v[f] - want).abs() < 1.0 / 8_388_608.0, "{h:?} {f}");
    }
}

#[test]
fn negotiation_refuses_what_the_driver_cannot_do() {
    let drv = Scripted::new(Script::default());
    let mut r = DuplexRequest::new(vec![0], 0, level());
    r.buffer_frames = Some(300);
    assert!(matches!(
        open(&drv, r),
        Err(AudioError::Unsupported(Unsupported::BufferFrames {
            requested: 300,
            ..
        }))
    ));
    let mut r = DuplexRequest::new(vec![0], 0, level());
    r.sample_rate = Some(96_000);
    match open(&drv, r) {
        Err(AudioError::Unsupported(Unsupported::SampleRate { offered, .. })) => {
            assert_eq!(offered, "44100 Hz, 48000 Hz");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        open(&drv, DuplexRequest::new(vec![4], 0, level())),
        Err(AudioError::Unsupported(Unsupported::InputChannel {
            channel: 4,
            available: 4
        }))
    ));
    assert!(matches!(
        open(&drv, DuplexRequest::new(vec![0], 3, level())),
        Err(AudioError::Unsupported(Unsupported::OutputChannels { .. }))
    ));
    // Another rate the driver offers is set on it.
    let mut r = DuplexRequest::new(vec![0], 0, level());
    r.sample_rate = Some(44_100);
    r.buffer_frames = Some(512);
    let s = open(&drv, r).expect("open");
    assert_eq!(s.negotiated().sample_rate, 44_100);
    assert_eq!(s.negotiated().buffer_frames, Some(512));
    drop(s);
    assert!(
        drv.calls
            .lock()
            .expect("scripted state")
            .contains(&"set_sample_rate")
    );

    let mut script = Script::default();
    script.inputs[2].sample_type = 1;
    let drv = Scripted::new(script);
    match open(&drv, DuplexRequest::new(vec![2], 0, level())) {
        Err(AudioError::Backend { detail, .. }) => {
            assert!(
                detail.contains("Int24MSB") && detail.contains("ADAT 1"),
                "{detail}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn input_and_output_name_one_driver() {
    let id = |s: &str| DeviceSelector::Id(DeviceId(s.into()));
    assert_eq!(
        one_device(&DeviceSelector::Default, &DeviceSelector::Default).expect("ok"),
        None
    );
    assert_eq!(
        one_device(&id("A"), &DeviceSelector::Default).expect("ok"),
        Some(DeviceId("A".into()))
    );
    assert_eq!(
        one_device(&DeviceSelector::Default, &id("A")).expect("ok"),
        Some(DeviceId("A".into()))
    );
    assert!(matches!(
        one_device(&id("A"), &id("B")),
        Err(AudioError::Unsupported(Unsupported::SeparateDevices { .. }))
    ));
}

/// Scripted drivers as installed drivers: what [`AsioHost`] lists, loads and opens. Every
/// load of a name gets the same driver, which keeps its settings across loads as hardware
/// does.
struct Installed {
    drivers: Vec<(String, Arc<Scripted>)>,
    slot: &'static Slot,
    creates: AtomicUsize,
}

impl std::fmt::Debug for Installed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Installed")
    }
}

impl Installed {
    fn new(scripts: Vec<(&str, Script)>) -> Arc<Self> {
        let slot: &'static Slot = Box::leak(Box::new(Slot::new()));
        Arc::new(Self {
            drivers: scripts
                .into_iter()
                .map(|(n, s)| (n.to_owned(), Scripted::with_slot(s, slot)))
                .collect(),
            slot,
            creates: AtomicUsize::new(0),
        })
    }

    fn driver(&self, name: &str) -> Arc<Scripted> {
        let d = self
            .drivers
            .iter()
            .find(|(n, _)| n == name)
            .expect("driver");
        Arc::clone(&d.1)
    }
}

impl DriverSource for Arc<Installed> {
    fn installed(&self) -> Result<Vec<String>, AudioError> {
        Ok(self.drivers.iter().map(|(n, _)| n.clone()).collect())
    }
    fn create(&self, name: &str) -> Result<Arc<dyn AsioDriver>, AudioError> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        Ok(self.driver(name) as Arc<dyn AsioDriver>)
    }
    fn slot(&self) -> &'static Slot {
        self.slot
    }
}

fn on(name: &str, req: DuplexRequest) -> DuplexRequest {
    let id = DeviceSelector::Id(DeviceId(name.into()));
    DuplexRequest {
        input_device: id.clone(),
        output_device: id,
        ..req
    }
}

fn unavailable(r: Result<DuplexStream, AudioError>) -> (Unavailability, String) {
    match r {
        Err(e @ AudioError::Unavailable { .. }) => {
            let text = e.to_string();
            let AudioError::Unavailable { reason, .. } = e else {
                unreachable!()
            };
            (reason, text)
        }
        Err(e) => panic!("not unavailable: {e:?}"),
        Ok(_) => panic!("opened"),
    }
}

#[test]
fn a_driver_held_by_another_program_is_named_in_use() {
    let rig = Installed::new(vec![("UMC ASIO Driver", Script::default())]);
    let host = AsioHost::new(Arc::clone(&rig));
    let drv = rig.driver("UMC ASIO Driver");

    // Refused at `init` without a word.
    *drv.init_refusal.lock().expect("scripted state") = Some(String::new());
    let (reason, text) = unavailable(host.open(DuplexRequest::new(vec![0], 0, level())));
    assert_eq!(
        reason,
        Unavailability::AsioDriverInUse {
            driver: "UMC ASIO Driver".into()
        }
    );
    assert!(
        text.ends_with(
            "UMC ASIO Driver is in use by another application (close the DAW or other audio \
             program using it)"
        ),
        "{text}"
    );

    // Loaded, then refused buffers.
    *drv.init_refusal.lock().expect("scripted state") = None;
    *drv.create_refusal.lock().expect("scripted state") = Some(AsioCode::HwMalfunction);
    let (reason, _) = unavailable(host.open(DuplexRequest::new(vec![0], 0, level())));
    assert!(
        matches!(reason, Unavailability::AsioDriverInUse { .. }),
        "{reason:?}"
    );

    // Released by the other program: the next open runs.
    *drv.create_refusal.lock().expect("scripted state") = None;
    let s = host
        .open(DuplexRequest::new(vec![0], 0, level()))
        .expect("open");
    assert_eq!(s.negotiated().input_device.0, "UMC ASIO Driver");
}

#[test]
fn an_interface_that_is_not_there_is_named_with_the_drivers_words() {
    let rig = Installed::new(vec![("Fireface USB", Script::default())]);
    let host = AsioHost::new(Arc::clone(&rig));
    let drv = rig.driver("Fireface USB");
    drv.present.store(false, Ordering::SeqCst);
    let (reason, text) = unavailable(host.open(DuplexRequest::new(vec![0], 0, level())));
    assert_eq!(
        reason,
        Unavailability::AsioHardwareNotPresent {
            driver: "Fireface USB".into(),
            detail: "No ASIO device found".into(),
        }
    );
    assert!(
        text.ends_with(
            "Fireface USB is installed but its interface is not present (connect it and switch \
             it on); the driver says: No ASIO device found"
        ),
        "{text}"
    );
    // A driver that loads but answers ASE_NotPresent to its first call.
    drv.present.store(true, Ordering::SeqCst);
    *drv.init_refusal.lock().expect("scripted state") = None;
    let s = host.open(DuplexRequest::new(vec![0], 0, level()));
    assert!(s.is_ok());
    drop(s);
    drv.present.store(false, Ordering::SeqCst);
    let e = super::open::refusal(
        "Fireface USB",
        super::open::Step::Other(crate::Operation::Open),
        drv.channel_counts().expect_err("unplugged"),
    );
    assert!(
        matches!(
            e,
            AudioError::Unavailable {
                reason: Unavailability::AsioHardwareNotPresent { .. },
                ..
            }
        ),
        "{e:?}"
    );
}

#[test]
fn an_unplugged_interface_ends_the_stream_and_reopens_once_back() {
    let rig = Installed::new(vec![("Fireface USB", Script::default())]);
    let host = AsioHost::new(Arc::clone(&rig));
    let drv = rig.driver("Fireface USB");
    let mut s = host
        .open(DuplexRequest::new(vec![0, 1], 2, level()))
        .expect("open");
    drv.tick();
    drv.tick();
    assert_eq!(blocks(&mut s).len(), 2);

    // Unplugged: the driver asks for a reset and stops switching.
    drv.present.store(false, Ordering::SeqCst);
    drv.message(selector::RESET_REQUEST, 0);
    assert!(s.events().ended);
    assert_eq!(s.events().reset, Some(ResetReason::ResetRequest));
    drop(s);
    assert!(!drv.slot.is_set());

    // Every attempt while it is away fails as not present; each is a fresh load.
    for _ in 0..3 {
        let (reason, _) = unavailable(host.open(DuplexRequest::new(vec![0, 1], 2, level())));
        assert!(
            matches!(reason, Unavailability::AsioHardwareNotPresent { .. }),
            "{reason:?}"
        );
    }
    // The registry still lists it: the look between attempts says present.
    assert!(matches!(
        host.probe(&DeviceSelector::Default),
        crate::Presence::Present { .. }
    ));

    // Plugged back in: the next attempt opens, indexed from 0 again.
    drv.present.store(true, Ordering::SeqCst);
    let mut s = host
        .open(DuplexRequest::new(vec![0, 1], 2, level()))
        .expect("reopen");
    drv.position.store(0, Ordering::SeqCst);
    drv.tick();
    let b = blocks(&mut s);
    assert_eq!(b[0].0.flags, BlockFlags::FIRST);
    assert_eq!(b[0].0.start_sample, 0);
    assert_eq!(rig.creates.load(Ordering::SeqCst), 5);
}

#[test]
fn a_panel_change_resets_the_stream_and_the_reopen_takes_the_drivers_new_rate() {
    let rig = Installed::new(vec![("Fireface USB", Script::default())]);
    let host = AsioHost::new(Arc::clone(&rig));
    let drv = rig.driver("Fireface USB");
    let s = host
        .open(DuplexRequest::new(vec![0], 2, level()))
        .expect("open");
    assert_eq!(
        (s.negotiated().sample_rate, s.negotiated().buffer_frames),
        (48_000, Some(256))
    );
    drv.tick();

    // The operator sets 44.1 kHz and 512 frames in the panel of the running driver: the
    // call goes to the stream's own driver, which asks for a reset.
    *drv.panel.lock().expect("scripted state") = Some((44_100.0, 512));
    host.control_panel(&DeviceSelector::Default).expect("panel");
    assert_eq!(drv.panel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(rig.creates.load(Ordering::SeqCst), 1, "no second load");
    let ev = s.events();
    assert!(
        ev.ended && ev.reset == Some(ResetReason::ResetRequest),
        "{ev:?}"
    );
    drop(s);

    // Reopened with no rate or buffer asked for: the driver's new ones.
    let s = host
        .open(DuplexRequest::new(vec![0], 2, level()))
        .expect("reopen");
    assert_eq!(
        (s.negotiated().sample_rate, s.negotiated().buffer_frames),
        (44_100, Some(512))
    );
    assert!(
        !drv.calls
            .lock()
            .expect("scripted state")
            .contains(&"set_sample_rate"),
        "the rate is the driver's, never set back"
    );
}

#[test]
fn the_panel_of_a_driver_with_no_stream_stays_loaded_and_the_open_reuses_it() {
    let rig = Installed::new(vec![
        ("Fireface USB", Script::default()),
        ("FlexASIO", Script::default()),
    ]);
    let host = AsioHost::new(Arc::clone(&rig));
    host.control_panel(&DeviceSelector::Id(DeviceId("FlexASIO".into())))
        .expect("panel");
    assert_eq!(rig.driver("FlexASIO").panel_calls.load(Ordering::SeqCst), 1);
    // Listing while the panel's driver is loaded loads nothing else.
    let list = host.enumerate().expect("list");
    assert_eq!(rig.creates.load(Ordering::SeqCst), 1);
    assert!(list[0].input.is_none(), "{:?}", list[0].notes);
    assert!(list[1].input.is_some());
    // Opening it keeps the instance the panel is showing.
    let s = host
        .open(on("FlexASIO", DuplexRequest::new(vec![0], 0, level())))
        .expect("open");
    assert_eq!(rig.creates.load(Ordering::SeqCst), 1);
    // Another driver's panel is refused while a stream runs on this one.
    assert!(matches!(
        host.control_panel(&DeviceSelector::Id(DeviceId("Fireface USB".into()))),
        Err(AudioError::Unavailable { .. })
    ));
    drop(s);
    host.control_panel(&DeviceSelector::Id(DeviceId("Fireface USB".into())))
        .expect("panel");
    assert_eq!(rig.creates.load(Ordering::SeqCst), 2);
}

#[test]
fn clock_sources_are_listed_with_the_current_one_and_named_at_open() {
    let ck = |n: &str, current| ClockDesc {
        name: n.into(),
        current,
    };
    let drv = Scripted::new(Script {
        clocks: Some(vec![
            ck("Internal", false),
            ck(" Word Clock ", true),
            ck("", false),
        ]),
        ..Script::default()
    });
    let c = device_caps(&*drv, "Scripted ASIO", true).expect("caps");
    assert_eq!(
        c.clock_sources,
        vec![
            ClockSource {
                name: "Internal".into(),
                current: false
            },
            ClockSource {
                name: "Word Clock".into(),
                current: true
            },
            ClockSource {
                name: "Clock 3".into(),
                current: false
            },
        ]
    );
    let s = open(&drv, DuplexRequest::new(vec![0], 0, level())).expect("open");
    assert_eq!(s.negotiated().clock_source.as_deref(), Some("Word Clock"));
    // A driver that lists none.
    let drv = Scripted::new(Script::default());
    assert!(
        device_caps(&*drv, "Scripted ASIO", true)
            .expect("caps")
            .clock_sources
            .is_empty()
    );
}

#[test]
fn a_sparse_input_map_makes_buffers_for_exactly_its_channels() {
    let ch = |n: &str| ChannelDesc {
        name: n.into(),
        sample_type: F32,
    };
    let drv = Scripted::new(Script {
        inputs: (1..=8).map(|i| ch(&format!("Analog {i}"))).collect(),
        ..Script::default()
    });
    // Inputs 3 and 7 only (device channels 2 and 6), in the operator's order 7, 3.
    let mut s = open(&drv, DuplexRequest::new(vec![6, 2], 2, level())).expect("open");
    let made: Vec<(bool, u16)> = drv
        .buffers
        .lock()
        .expect("scripted state")
        .channels
        .iter()
        .map(|&(i, c, _)| (i, c))
        .collect();
    assert_eq!(made, [(true, 2), (true, 6), (false, 0), (false, 1)]);
    assert_eq!(s.negotiated().input_channels, 2);
    assert_eq!(s.negotiated().device_input_channels, 8);
    drv.tick();
    drv.tick();
    for (h, v) in blocks(&mut s) {
        assert_eq!(h.channels, 2);
        let frames = v.len() / 2;
        for f in [0, frames - 1] {
            let pos = h.start_sample as i64 + f as i64;
            // The driver's position is the block index here (first switch at 256).
            let pos = pos + i64::from(FRAMES);
            assert_eq!(
                v[f * 2],
                Scripted::signal(6, pos),
                "block {}",
                h.start_sample
            );
            assert_eq!(v[f * 2 + 1], Scripted::signal(2, pos));
        }
    }
}

#[test]
fn driver_buffering_is_stated_when_the_driver_reports_it_and_not_otherwise() {
    let drv = Scripted::new(Script::default());
    let s = open(&drv, DuplexRequest::new(vec![0], 2, level())).expect("open");
    assert_eq!(
        s.negotiated().latency,
        StaticLatency::Frames {
            input: 32,
            output: 48,
            driver_buffering: None,
        },
        "a driver before ASIO 2.3 answers not present: not reported, never 0"
    );
    drop(s);
    *drv.internal.lock().expect("scripted state") = Some((16, 24));
    let s = open(&drv, DuplexRequest::new(vec![0], 2, level())).expect("reopen");
    assert_eq!(
        s.negotiated().latency,
        StaticLatency::Frames {
            input: 32,
            output: 48,
            driver_buffering: Some(DriverBuffering {
                input: 16,
                output: 24
            }),
        }
    );
}

/// A stream playing a sine on output 0, run until it is at full level.
fn emitting(drv: &Arc<Scripted>) -> (DuplexStream, GeneratorHandle) {
    let (mut handle, port) = generator([0]).expect("routes");
    handle
        .set_source(Box::new(Sine::new(1000.0, 48_000, 0.05)))
        .expect("queue");
    let mut req = DuplexRequest::new(vec![0], 2, level());
    req.output = OutputSource::Generator(port);
    let s = open(drv, req).expect("open");
    handle.start();
    // The 20 ms fade-in is under four periods of 256 at 48 kHz.
    for _ in 0..6 {
        drv.tick();
    }
    assert_eq!(handle.state(), OutputState::Active);
    (s, handle)
}

/// Peak of output 0 over frames `range` of the half the last switch wrote.
fn last_peak(drv: &Scripted, range: std::ops::Range<usize>) -> f32 {
    let half = drv.half.load(Ordering::SeqCst);
    range.fold(0.0, |m, f| m.max(drv.output(0, half, f).abs()))
}

#[test]
fn every_clock_event_cuts_the_output_within_one_block_and_names_itself() {
    type Event = fn(&Scripted);
    let cases: [(&str, Event, ResetReason); 6] = [
        (
            "reset request",
            |d| {
                d.message(selector::RESET_REQUEST, 0);
            },
            ResetReason::ResetRequest,
        ),
        (
            "resync request",
            |d| {
                d.message(selector::RESYNC_REQUEST, 0);
            },
            ResetReason::ResyncRequest,
        ),
        (
            "buffer size change",
            |d| {
                d.message(selector::BUFFER_SIZE_CHANGE, 512);
            },
            ResetReason::BufferSizeChange,
        ),
        (
            "another rate",
            |d| d.slot.with(|e| e.expect("engine").rate_changed(44_100.0)),
            ResetReason::RateChange,
        ),
        (
            "the same rate (a relock or another source)",
            |d| d.slot.with(|e| e.expect("engine").rate_changed(48_000.0)),
            ResetReason::RateChange,
        ),
        (
            "no clock",
            |d| d.slot.with(|e| e.expect("engine").rate_changed(0.0)),
            ResetReason::ClockLost,
        ),
    ];
    // 2 ms at 48 kHz.
    const CUT: usize = 96;
    for (what, event, reason) in cases {
        let drv = Scripted::new(Script::default());
        let (s, handle) = emitting(&drv);
        assert!(last_peak(&drv, 0..256) > 0.04, "{what}: playing");
        event(&drv);
        let ev = s.events();
        assert!(ev.ended, "{what}: the stream ends");
        assert_eq!(ev.reset, Some(reason), "{what}");
        // The next block ramps down over the cut and is silent after it.
        drv.tick();
        assert!(last_peak(&drv, 0..CUT / 2) > 0.0, "{what}: a ramp, no step");
        assert_eq!(
            last_peak(&drv, CUT..256),
            0.0,
            "{what}: silent after the cut"
        );
        assert_eq!(handle.state(), OutputState::Silent, "{what}");
        // Silent for good: starting the generator again on this stream does nothing.
        handle.start();
        drv.tick();
        drv.tick();
        assert_eq!(last_peak(&drv, 0..256), 0.0, "{what}: stays silent");
    }
}
