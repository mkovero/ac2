//! The ASIO backend against a scripted in-process driver: negotiation, the shared index,
//! repeated switches, messages and the slot's lifecycle, with no Windows and no COM.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::engine::{RtDriver, Slot, TimeInfo, answer, selector};
use super::format::Format;
use super::open::{AsioDriver, BufferSizes, ChannelDesc, device_caps, one_device, open_stream};
use crate::backend::{DeviceId, DeviceSelector, DuplexRequest, IndexExactness, StaticLatency};
use crate::block::{BlockFlags, BlockHeader};
use crate::error::{AudioError, Unsupported};
use crate::level::MaxLevel;
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

struct Scripted {
    script: Script,
    slot: &'static Slot,
    rate: Mutex<f64>,
    latencies: Mutex<(u32, u32)>,
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
        Arc::new(Self {
            script,
            slot: Box::leak(Box::new(Slot::new())),
            rate: Mutex::new(48_000.0),
            latencies: Mutex::new((32, 48)),
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

impl AsioDriver for Scripted {
    fn channel_counts(&self) -> Result<(u16, u16), String> {
        Ok((
            self.script.inputs.len() as u16,
            self.script.outputs.len() as u16,
        ))
    }
    fn channel(&self, input: bool, index: u16) -> Result<ChannelDesc, String> {
        let list = if input {
            &self.script.inputs
        } else {
            &self.script.outputs
        };
        list.get(usize::from(index))
            .cloned()
            .ok_or_else(|| "no such channel".into())
    }
    fn buffer_sizes(&self) -> Result<BufferSizes, String> {
        Ok(self.script.sizes)
    }
    fn can_sample_rate(&self, hz: f64) -> bool {
        self.script.rates.contains(&hz)
    }
    fn sample_rate(&self) -> Result<f64, String> {
        Ok(*self.rate.lock().expect("scripted state"))
    }
    fn set_sample_rate(&self, hz: f64) -> Result<(), String> {
        self.log("set_sample_rate");
        *self.rate.lock().expect("scripted state") = hz;
        Ok(())
    }
    fn latencies(&self) -> Result<(u32, u32), String> {
        Ok(*self.latencies.lock().expect("scripted state"))
    }
    fn can_report_overload(&self) -> bool {
        false
    }
    fn create_buffers(
        &self,
        channels: &[(bool, u16)],
        frames: u32,
    ) -> Result<Vec<[*mut u8; 2]>, String> {
        self.log("create_buffers");
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
    fn start(&self) -> Result<(), String> {
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
    fn stop(&self) -> Result<(), String> {
        self.log("stop");
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
    fn dispose_buffers(&self) -> Result<(), String> {
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
            output: 48
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
            output: 48
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
    drv.tick();
    drv.slot.with(|e| e.expect("engine").rate_changed(48_000.0));
    assert!(!s.events().ended, "the same rate is no change");
    drv.slot.with(|e| e.expect("engine").rate_changed(44_100.0));
    assert!(s.events().ended);
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
