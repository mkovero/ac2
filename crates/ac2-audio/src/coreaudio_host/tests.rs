//! The adapter against scripted HAL devices: listing, opening, the shared index, latency,
//! events, consent. Each cycle is driven by the test, as the HAL's I/O thread would.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cahal::{
    ChannelTable, ClockSource as HalClock, Cycle, CycleTime, DeviceProperty, Error, FrameRange,
    Indexer, Input, IoConfig, IoStats, Latency, Output, RateRange, Scope, StreamFormat, StreamInfo,
    StreamLayout, Transport,
};

use super::CoreAudioBackend;
use super::source::{Consent, CycleFn, DevDesc, HalSource, Notice, Notify, Running};
use crate::backend::{
    Backend, BackendKind, ClockRelation, DeviceId, DeviceSelector, DuplexRequest, IndexExactness,
    Presence, SampleFormat, StaticLatency,
};
use crate::block::BlockFlags;
use crate::error::{AudioError, Unavailability, Unsupported};
use crate::level::MaxLevel;
use crate::stream::DuplexStream;

fn stream(channels: u32) -> StreamInfo {
    StreamInfo {
        channels,
        interleaved: true,
        format: StreamFormat::Float32,
    }
}

fn layout(streams: &[u32]) -> StreamLayout {
    StreamLayout {
        streams: streams.iter().map(|&c| stream(c)).collect(),
    }
}

/// A two-in, two-out virtual device like BlackHole.
fn blackhole() -> DevDesc {
    DevDesc {
        uid: "BlackHole2ch_UID".into(),
        name: "BlackHole 2ch".into(),
        object_id: 77,
        transport: Transport::Virtual,
        input: layout(&[2]),
        output: layout(&[2]),
        input_names: vec![None, None],
        output_names: vec![None, None],
        nominal_rate: 48_000.0,
        rates: vec![
            RateRange {
                min: 44_100.0,
                max: 44_100.0,
            },
            RateRange {
                min: 48_000.0,
                max: 48_000.0,
            },
        ],
        buffer_frames: 512,
        buffer_range: FrameRange { min: 15, max: 4096 },
        variable_buffer: None,
        input_latency: Latency::default(),
        output_latency: Latency::default(),
        clock_sources: Vec::new(),
        hog_pid: None,
    }
}

/// An interface with two input streams (analog 1–2, ADAT 3–10) and named channels.
fn interface() -> DevDesc {
    let mut names: Vec<Option<String>> = vec![Some("Analog 1".into()), Some("Analog 2".into())];
    names.extend((3..=10).map(|i| Some(format!("ADAT {}", i - 2))));
    DevDesc {
        uid: "AppleUSBAudioEngine:Vendor:Box:1234:1".into(),
        name: "Box".into(),
        object_id: 90,
        transport: Transport::Usb,
        input: layout(&[2, 8]),
        output: layout(&[2]),
        input_names: names,
        output_names: vec![Some("Out".into()), Some("Out".into())],
        clock_sources: vec![
            HalClock {
                id: 1,
                name: "Internal".into(),
                current: true,
            },
            HalClock {
                id: 2,
                name: " ".into(),
                current: false,
            },
        ],
        ..blackhole()
    }
}

/// A USB microphone: input only.
fn mic() -> DevDesc {
    DevDesc {
        uid: "mic".into(),
        name: "Mic".into(),
        object_id: 5,
        transport: Transport::Usb,
        output: StreamLayout::default(),
        output_names: Vec::new(),
        input: layout(&[1]),
        input_names: vec![None],
        ..blackhole()
    }
}

/// Built-in speakers: output only.
fn speakers() -> DevDesc {
    DevDesc {
        uid: "speakers".into(),
        name: "Speakers".into(),
        object_id: 6,
        transport: Transport::BuiltIn,
        input: StreamLayout::default(),
        input_names: Vec::new(),
        ..blackhole()
    }
}

/// One started IOProc as the test drives it.
struct ScriptRun {
    uid: String,
    config: IoConfig,
    input: ChannelTable,
    output: ChannelTable,
    f: Mutex<CycleFn>,
    indexer: Mutex<Indexer>,
    stats: Mutex<IoStats>,
    dead: AtomicBool,
    /// Never calls back: `wait_first_cycle` times out.
    silent: bool,
    /// out − in sample time of the priming cycle.
    out_minus_in: f64,
    origin: f64,
    stopped: Arc<AtomicBool>,
}

impl ScriptRun {
    /// One cycle at input sample time `t` (`None`: flagged invalid). `input[b]` is the
    /// content of buffer `b`; returns the output buffers.
    fn cycle(
        &self,
        t: Option<f64>,
        frames: usize,
        overloads: u32,
        input: &[Vec<f32>],
    ) -> Vec<Vec<f32>> {
        assert!(!self.stopped.load(Ordering::SeqCst), "cycle after stop");
        let ins: Vec<&[f32]> = (0..self.input.buffers().len())
            .map(|b| input.get(b).map_or(&[][..], |v| &v[..]))
            .collect();
        let mut outs: Vec<Vec<f32>> = self
            .output
            .buffers()
            .iter()
            .map(|&c| vec![9.0; frames * c as usize])
            .collect();
        let mut out_refs: Vec<&mut [f32]> = outs.iter_mut().map(|v| &mut v[..]).collect();
        let mut out = Output::new(&mut out_refs, &self.output, frames);
        out.silence();
        if self.dead.load(Ordering::SeqCst) {
            return outs;
        }
        let (index, gap) = self.indexer.lock().expect("lock").advance(t, frames as u32);
        let estimated = self.indexer.lock().expect("lock").estimated();
        let cycle = Cycle {
            input: Input::new(&ins, &self.input, frames),
            output: out,
            time: CycleTime {
                index,
                frames: frames as u32,
                gap,
                estimated,
                overloads,
                input_sample_time: t,
                output_sample_time: t.map(|t| t + self.out_minus_in),
                now_ns: 1_000_000 + index,
                input_ns: Some(index),
                output_ns: Some(index + 1),
                rate_scalar: 1.0,
            },
        };
        let mut f = self.f.lock().expect("lock");
        let r = catch_unwind(AssertUnwindSafe(|| (f)(cycle)));
        let mut s = self.stats.lock().expect("lock");
        s.cycles += 1;
        if r.is_err() {
            s.panicked = true;
            self.dead.store(true, Ordering::SeqCst);
            for o in &mut outs {
                o.fill(0.0);
            }
        }
        outs
    }
}

struct Handle(Arc<ScriptRun>);

impl Running for Handle {
    fn stats(&self) -> IoStats {
        *self.0.stats.lock().expect("lock")
    }
    fn wait_first_cycle(&self, _timeout: Duration) -> Result<(), Error> {
        if self.0.silent {
            return Err(Error::StartTimedOut);
        }
        if self.0.stats.lock().expect("lock").cycles == 0 {
            let frames = 512;
            let bufs: Vec<Vec<f32>> = self
                .0
                .input
                .buffers()
                .iter()
                .map(|&c| vec![0.0; frames * c as usize])
                .collect();
            self.0.cycle(Some(self.0.origin), frames, 0, &bufs);
        }
        Ok(())
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.stopped.store(true, Ordering::SeqCst);
    }
}

struct Scripted {
    devices: Mutex<Vec<DevDesc>>,
    default_in: Option<String>,
    default_out: Option<String>,
    /// What `microphone()` says, and what a request answers.
    consent: Mutex<(Consent, Consent)>,
    requests: Mutex<Vec<Duration>>,
    runs: Mutex<Vec<Arc<ScriptRun>>>,
    notifies: Mutex<Vec<Notify>>,
    rates_set: Mutex<Vec<(String, f64)>>,
    buffers_set: Mutex<Vec<(String, u32)>>,
    silent: AtomicBool,
    out_minus_in: Mutex<Option<f64>>,
}

impl Scripted {
    fn new(devices: Vec<DevDesc>) -> Arc<Self> {
        let first = devices.first().map(|d| d.uid.clone());
        Arc::new(Self {
            default_in: first.clone(),
            default_out: first,
            devices: Mutex::new(devices),
            consent: Mutex::new((Consent::Authorized, Consent::Authorized)),
            requests: Mutex::default(),
            runs: Mutex::default(),
            notifies: Mutex::default(),
            rates_set: Mutex::default(),
            buffers_set: Mutex::default(),
            silent: AtomicBool::new(false),
            out_minus_in: Mutex::new(None),
        })
    }

    fn run(&self, i: usize) -> Arc<ScriptRun> {
        Arc::clone(&self.runs.lock().expect("lock")[i])
    }

    fn notify(&self, n: Notice) {
        for f in self.notifies.lock().expect("lock").iter() {
            f(n);
        }
    }

    fn set_device(&self, uid: &str, f: impl FnOnce(&mut DevDesc)) {
        let mut d = self.devices.lock().expect("lock");
        f(d.iter_mut().find(|d| d.uid == uid).expect("device"));
    }
}

impl HalSource for Scripted {
    fn uids(&self) -> Result<Vec<String>, Error> {
        Ok(self
            .devices
            .lock()
            .expect("lock")
            .iter()
            .map(|d| d.uid.clone())
            .collect())
    }
    fn default_uid(&self, scope: Scope) -> Result<Option<String>, Error> {
        Ok(match scope {
            Scope::Input => self.default_in.clone(),
            Scope::Output => self.default_out.clone(),
        })
    }
    fn object_id(&self, uid: &str) -> Result<Option<u32>, Error> {
        Ok(self
            .devices
            .lock()
            .expect("lock")
            .iter()
            .find(|d| d.uid == uid)
            .map(|d| d.object_id))
    }
    fn describe(&self, uid: &str) -> Result<DevDesc, Error> {
        self.devices
            .lock()
            .expect("lock")
            .iter()
            .find(|d| d.uid == uid)
            .cloned()
            .ok_or(Error::NotFound)
    }
    fn set_rate(&self, uid: &str, hz: f64) -> Result<(), Error> {
        self.rates_set.lock().expect("lock").push((uid.into(), hz));
        self.set_device(uid, |d| d.nominal_rate = hz);
        Ok(())
    }
    fn set_buffer(&self, uid: &str, frames: u32) -> Result<(), Error> {
        self.buffers_set
            .lock()
            .expect("lock")
            .push((uid.into(), frames));
        self.set_device(uid, |d| d.buffer_frames = frames);
        Ok(())
    }
    fn start(
        &self,
        uid: &str,
        config: IoConfig,
        f: CycleFn,
        notify: Notify,
    ) -> Result<Box<dyn Running>, Error> {
        let d = self.describe(uid)?;
        let table = |used: bool, l: &StreamLayout| {
            if used {
                ChannelTable::new(l)
            } else {
                ChannelTable::default()
            }
        };
        let run = Arc::new(ScriptRun {
            uid: uid.into(),
            config,
            input: table(config.input, &d.input),
            output: table(config.output, &d.output),
            f: Mutex::new(f),
            indexer: Mutex::new(Indexer::new()),
            stats: Mutex::new(IoStats::default()),
            dead: AtomicBool::new(false),
            silent: self.silent.load(Ordering::SeqCst),
            out_minus_in: self
                .out_minus_in
                .lock()
                .expect("lock")
                .unwrap_or(2.0 * f64::from(d.buffer_frames)),
            origin: 1_000_000.0,
            stopped: Arc::new(AtomicBool::new(false)),
        });
        self.runs.lock().expect("lock").push(Arc::clone(&run));
        self.notifies.lock().expect("lock").push(notify);
        Ok(Box::new(Handle(run)))
    }
    fn microphone(&self) -> Consent {
        self.consent.lock().expect("lock").0
    }
    fn request_microphone(&self, timeout: Duration) -> Consent {
        self.requests.lock().expect("lock").push(timeout);
        self.consent.lock().expect("lock").1
    }
}

fn backend(s: &Arc<Scripted>) -> CoreAudioBackend {
    CoreAudioBackend::with_source(Arc::clone(s) as Arc<dyn HalSource>)
}

fn id(uid: &str) -> DeviceSelector {
    DeviceSelector::Id(DeviceId(format!("coreaudio:{uid}")))
}

fn request(map: Vec<u16>, outputs: u16) -> DuplexRequest {
    DuplexRequest::new(map, outputs, MaxLevel::from_peak_db(-20.0).expect("level"))
}

/// Pops every block: (start, frames, flags, samples).
fn drain(s: &mut DuplexStream) -> Vec<(u64, u32, BlockFlags, Vec<f32>)> {
    let mut v = Vec::new();
    while let Some(()) = s.capture().pop_with(|h, a, b| {
        v.push((h.start_sample, h.frames, h.flags, [a, b].concat()));
    }) {}
    v
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(
            t.elapsed() < Duration::from_secs(3),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn lists_devices_with_exact_index_and_named_channels() {
    let s = Scripted::new(vec![blackhole(), interface(), mic()]);
    let caps = backend(&s).enumerate().expect("list");
    assert_eq!(caps.len(), 3);
    let bh = &caps[0];
    assert_eq!(bh.backend, BackendKind::CoreAudio);
    assert_eq!(bh.host, "coreaudio");
    assert_eq!(bh.id.0, "coreaudio:BlackHole2ch_UID");
    assert_eq!(bh.duplex_clock, ClockRelation::SingleCallback);
    assert_eq!(bh.index, IndexExactness::Exact);
    // BlackHole: no latencies or safety offsets, out − in two buffers.
    assert_eq!(
        bh.latency,
        StaticLatency::Frames {
            input: 512,
            output: 512,
            driver_buffering: None,
        }
    );
    let input = bh.input.as_ref().expect("input");
    assert_eq!(input.max_channels, 2);
    assert_eq!(input.formats, vec![SampleFormat::F32]);
    assert_eq!(input.default_rate, Some(48_000));
    assert_eq!(input.default_buffer, Some(512));
    assert_eq!(
        input.buffer_frames,
        Some(crate::backend::FrameRange { min: 15, max: 4096 })
    );
    assert_eq!(input.rates.len(), 2);
    assert!(input.system_default);
    assert_eq!(input.channel_names, None, "unnamed channels are numbered");

    let ifc = &caps[1];
    let names = ifc
        .input
        .as_ref()
        .and_then(|i| i.channel_names.clone())
        .expect("names");
    assert_eq!(names.len(), 10);
    assert_eq!(
        (names[0].as_str(), names[9].as_str()),
        ("Analog 1", "ADAT 8")
    );
    assert_eq!(
        ifc.output.as_ref().and_then(|o| o.channel_names.clone()),
        None,
        "repeated names are numbered"
    );
    assert!(!ifc.input.as_ref().expect("in").system_default);
    let clocks: Vec<_> = ifc
        .clock_sources
        .iter()
        .map(|c| (c.name.as_str(), c.current))
        .collect();
    assert_eq!(clocks, vec![("Internal", true), ("Clock 2", false)]);

    assert_eq!(caps[2].duplex_clock, ClockRelation::Unknown);
    assert!(caps[2].output.is_none());
}

#[test]
fn notes_name_what_ac2_cannot_use_or_trust() {
    let mut d = blackhole();
    d.output.streams[0].format = StreamFormat::Other {
        format_id: u32::from_be_bytes(*b"lpcm"),
        flags: 0xc,
        bits: 24,
    };
    d.variable_buffer = Some(1024);
    d.hog_pid = Some(1);
    d.transport = Transport::Aggregate;
    let s = Scripted::new(vec![d]);
    let caps = backend(&s).enumerate().expect("list");
    let notes = caps[0].notes.join("\n");
    assert!(
        notes.contains("output stream 1 is 'lpcm' (24 bit"),
        "{notes}"
    );
    assert!(
        notes.contains("variable buffer sizes (up to 1024"),
        "{notes}"
    );
    assert!(notes.contains("hog mode) by process 1"), "{notes}");
    assert!(notes.contains("aggregate"), "{notes}");
    assert!(caps[0].output.as_ref().expect("out").formats.is_empty());
}

#[test]
fn stated_latency_adds_buffers_offsets_and_converters() {
    let mut d = blackhole();
    d.input_latency = Latency {
        device: 10,
        safety_offset: 16,
        streams: vec![3],
    };
    d.output_latency = Latency {
        device: 20,
        safety_offset: 24,
        streams: vec![4],
    };
    let s = Scripted::new(vec![d]);
    // As the HAL places it: 2 buffers plus both safety offsets.
    *s.out_minus_in.lock().expect("lock") = Some(2.0 * 512.0 + 16.0 + 24.0);
    let mut r = request(vec![0], 2);
    r.buffer_frames = Some(512);
    let st = backend(&s).open(r).expect("open");
    // input = buffer + safety_in + latency_in + stream_in;
    // output = (out − in) − buffer − safety_in + latency_out + stream_out.
    assert_eq!(
        st.negotiated().latency,
        StaticLatency::Frames {
            input: 512 + 16 + 10 + 3,
            output: 1064 - 512 - 16 + 20 + 4,
            driver_buffering: None,
        }
    );
    // Listing states the same from the HAL's placement, before any cycle.
    assert_eq!(
        backend(&s).enumerate().expect("list")[0].latency,
        st.negotiated().latency
    );
}

#[test]
fn input_and_output_share_the_index_from_the_sample_time() {
    let s = Scripted::new(vec![blackhole()]);
    let mut st = backend(&s).open(request(vec![1, 0], 2)).expect("open");
    let n = st.negotiated().clone();
    assert_eq!(n.backend, BackendKind::CoreAudio);
    assert_eq!(n.clock, ClockRelation::SingleCallback);
    assert_eq!(n.index, IndexExactness::Exact);
    assert_eq!(n.input_device.0, "coreaudio:BlackHole2ch_UID");
    assert_eq!(n.output_device, n.input_device);
    assert_eq!((n.sample_rate, n.buffer_frames), (48_000, Some(1024)));
    assert_eq!(n.device_output_channels, 2);
    assert_eq!(n.input_format, SampleFormat::F32);
    let run = s.run(0);
    assert_eq!(
        run.config,
        IoConfig {
            input: true,
            output: true
        }
    );
    // The priming cycle ran at sample time 1e6 with 512 frames; the device now runs 1024.
    let buf: Vec<f32> = (0..2048).map(|i| i as f32).collect();
    run.cycle(Some(1_000_512.0), 1024, 0, std::slice::from_ref(&buf));
    run.cycle(Some(1_001_536.0 + 4096.0), 1024, 0, &[buf]);
    let blocks = drain(&mut st);
    assert_eq!(blocks.len(), 3);
    assert_eq!((blocks[0].0, blocks[0].2), (0, BlockFlags::FIRST));
    assert_eq!((blocks[1].0, blocks[1].2), (512, BlockFlags::NONE));
    // The input map swaps the two device channels.
    assert_eq!(&blocks[1].3[..4], &[1.0, 0.0, 3.0, 2.0]);
    // A sample-time jump of 4096 is exactly 4096 frames lost.
    assert_eq!(
        (blocks[2].0, blocks[2].2),
        (1536 + 4096, BlockFlags::DISCONTINUITY)
    );
    let mut ticks = Vec::new();
    while let Some(t) = st.pop_output_tick() {
        ticks.push(t.start_sample);
    }
    assert_eq!(
        ticks,
        vec![0, 512, 1536 + 4096],
        "output ticks on the same index"
    );
    st.stop(Duration::from_secs(1));
    assert!(run.stopped.load(Ordering::SeqCst));
}

#[test]
fn sparse_inputs_across_two_streams() {
    let s = Scripted::new(vec![interface()]);
    let mut st = backend(&s).open(request(vec![1, 6], 0)).expect("open");
    assert_eq!(st.negotiated().device_input_channels, 10);
    let run = s.run(0);
    assert!(!run.config.output);
    let analog: Vec<f32> = (0..512 * 2)
        .map(|i| if i % 2 == 1 { 0.5 } else { 0.0 })
        .collect();
    let adat: Vec<f32> = (0..512 * 8)
        .map(|i| if i % 8 == 4 { 0.25 } else { 0.0 })
        .collect();
    run.cycle(Some(1_000_512.0), 512, 0, &[analog, adat]);
    let blocks = drain(&mut st);
    let last = &blocks.last().expect("block").3;
    assert_eq!(&last[..4], &[0.5, 0.25, 0.5, 0.25]);
}

#[test]
fn an_overload_is_an_xrun_on_that_block() {
    let s = Scripted::new(vec![blackhole()]);
    let mut st = backend(&s).open(request(vec![0], 2)).expect("open");
    let run = s.run(0);
    let z = vec![0.0; 2048];
    run.cycle(Some(1_000_512.0), 1024, 1, std::slice::from_ref(&z));
    run.cycle(Some(1_001_536.0), 1024, 0, &[z]);
    let blocks = drain(&mut st);
    assert!(blocks[1].2.contains(BlockFlags::XRUN), "{:?}", blocks[1].2);
    assert!(!blocks[2].2.contains(BlockFlags::XRUN));
    assert_eq!(st.events().xruns, 1);
}

#[test]
fn an_invalid_sample_time_flags_an_estimated_gap() {
    let s = Scripted::new(vec![blackhole()]);
    let mut st = backend(&s).open(request(vec![0], 0)).expect("open");
    let run = s.run(0);
    run.cycle(None, 1024, 0, &[vec![0.0; 2048]]);
    let blocks = drain(&mut st);
    assert_eq!(
        blocks[1].2,
        BlockFlags::DISCONTINUITY | BlockFlags::GAP_ESTIMATED
    );
    assert_eq!(blocks[1].0, 512);
}

#[test]
fn the_rate_is_set_only_when_asked_for_another() {
    let s = Scripted::new(vec![blackhole()]);
    drop(backend(&s).open(request(vec![0], 0)).expect("open"));
    assert!(s.rates_set.lock().expect("lock").is_empty());
    let mut r = request(vec![0], 0);
    r.sample_rate = Some(44_100);
    let st = backend(&s).open(r).expect("open");
    assert_eq!(st.negotiated().sample_rate, 44_100);
    assert_eq!(
        *s.rates_set.lock().expect("lock"),
        vec![("BlackHole2ch_UID".to_string(), 44_100.0)]
    );
    drop(st);
    let mut r = request(vec![0], 0);
    r.sample_rate = Some(96_000);
    let e = backend(&s).open(r).expect_err("96 kHz");
    assert!(
        matches!(e, AudioError::Unsupported(Unsupported::SampleRate { requested: 96_000, ref offered }) if offered == "44100 Hz, 48000 Hz"),
        "{e}"
    );
    let mut r = request(vec![0], 0);
    r.buffer_frames = Some(8192);
    assert!(matches!(
        backend(&s).open(r),
        Err(AudioError::Unsupported(Unsupported::BufferFrames {
            requested: 8192,
            ..
        }))
    ));
}

#[test]
fn unplugging_ends_the_stream() {
    for n in [
        Notice::Device(DeviceProperty::Alive),
        Notice::Device(DeviceProperty::IoStoppedAbnormally),
        Notice::ServiceRestarted,
    ] {
        let s = Scripted::new(vec![blackhole()]);
        let st = backend(&s).open(request(vec![0], 2)).expect("open");
        assert!(!st.events().ended);
        s.notify(n);
        wait_until("end", || st.events().ended);
        assert!(st.events().reset.is_none());
    }
}

#[test]
fn a_rate_change_by_someone_else_resets_the_stream() {
    let s = Scripted::new(vec![blackhole()]);
    let st = backend(&s).open(request(vec![0], 2)).expect("open");
    // A notification with the rate unchanged is not a reset.
    s.notify(Notice::Device(DeviceProperty::NominalRate));
    std::thread::sleep(Duration::from_millis(250));
    assert!(st.events().reset.is_none());
    s.set_device("BlackHole2ch_UID", |d| d.nominal_rate = 44_100.0);
    s.notify(Notice::Device(DeviceProperty::NominalRate));
    wait_until("reset", || st.events().reset.is_some());
    // So is a new stream layout or clock source.
    for p in [
        DeviceProperty::StreamConfiguration,
        DeviceProperty::ClockSource,
    ] {
        let s = Scripted::new(vec![blackhole()]);
        let st = backend(&s).open(request(vec![0], 2)).expect("open");
        s.notify(Notice::Device(p));
        wait_until("reset", || st.events().reset.is_some());
    }
}

#[test]
fn a_changed_latency_is_a_config_change_only_when_it_differs() {
    let s = Scripted::new(vec![blackhole()]);
    let mut st = backend(&s).open(request(vec![0], 2)).expect("open");
    s.notify(Notice::Device(DeviceProperty::Latency));
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(st.events().config_changes, 0);
    s.set_device("BlackHole2ch_UID", |d| d.output_latency.device = 33);
    s.notify(Notice::Device(DeviceProperty::Latency));
    wait_until("config change", || st.events().config_changes == 1);
    // The next block carries it.
    s.run(0)
        .cycle(Some(1_000_512.0), 1024, 0, &[vec![0.0; 2048]]);
    let blocks = drain(&mut st);
    assert!(
        blocks
            .last()
            .expect("block")
            .2
            .contains(BlockFlags::CONFIG_CHANGE)
    );
}

#[test]
fn a_panicking_cycle_is_silent_and_ends_the_stream() {
    let s = Scripted::new(vec![blackhole()]);
    let st = backend(&s).open(request(vec![0], 2)).expect("open");
    let run = s.run(0);
    // The closure panics (as the IOProc trampoline catches it: outputs zeroed, never
    // called again, `IoStats::panicked`).
    let out = {
        let mut f = run.f.lock().expect("lock");
        *f = Box::new(|_c| panic!("scripted panic"));
        drop(f);
        run.cycle(Some(1_000_512.0), 1024, 0, &[vec![0.0; 2048]])
    };
    assert!(out.iter().all(|b| b.iter().all(|&v| v == 0.0)));
    wait_until("end", || st.events().ended);
    // Not called again.
    let out = run.cycle(Some(1_001_536.0), 1024, 0, &[vec![0.0; 2048]]);
    assert!(out.iter().all(|b| b.iter().all(|&v| v == 0.0)));
}

#[test]
fn consent_is_checked_before_any_capture_starts() {
    let s = Scripted::new(vec![blackhole()]);
    *s.consent.lock().expect("lock") = (Consent::Denied, Consent::Denied);
    let e = backend(&s).open(request(vec![0], 2)).expect_err("denied");
    assert!(
        matches!(
            e,
            AudioError::Unavailable {
                backend: BackendKind::CoreAudio,
                reason: Unavailability::MicrophoneDenied
            }
        ),
        "{e}"
    );
    assert!(e.to_string().contains("Privacy & Security"), "{e}");
    assert!(s.runs.lock().expect("lock").is_empty(), "nothing started");
    assert!(
        s.requests.lock().expect("lock").is_empty(),
        "a decided answer is not asked again"
    );

    *s.consent.lock().expect("lock") = (Consent::NotDetermined, Consent::NotDetermined);
    let e = backend(&s)
        .with_consent_wait(Duration::from_millis(5))
        .open(request(vec![0], 2))
        .expect_err("pending");
    assert!(
        matches!(
            e,
            AudioError::Unavailable {
                reason: Unavailability::MicrophonePending,
                ..
            }
        ),
        "{e}"
    );
    assert_eq!(
        *s.requests.lock().expect("lock"),
        vec![Duration::from_millis(5)]
    );

    *s.consent.lock().expect("lock") = (Consent::NotDetermined, Consent::Authorized);
    backend(&s)
        .open(request(vec![0], 2))
        .expect("granted at the prompt");
}

#[test]
fn a_device_that_never_calls_back_fails_the_open() {
    let s = Scripted::new(vec![blackhole()]);
    s.silent.store(true, Ordering::SeqCst);
    let e = backend(&s).open(request(vec![0], 2)).expect_err("silent");
    assert!(e.to_string().contains("did not start calling back"), "{e}");
    assert!(
        s.run(0).stopped.load(Ordering::SeqCst),
        "the IOProc is stopped"
    );
}

#[test]
fn two_devices_run_two_indexes_on_unknown_clocks() {
    let s = Scripted::new(vec![mic(), speakers()]);
    let mut r = request(vec![0], 2);
    r.input_device = id("mic");
    r.output_device = id("speakers");
    let mut st = backend(&s).open(r).expect("open");
    let n = st.negotiated().clone();
    assert_eq!(n.clock, ClockRelation::Unknown);
    assert_eq!(n.output_device.0, "coreaudio:speakers");
    let (out, inp) = (s.run(0), s.run(1));
    assert_eq!((out.uid.as_str(), inp.uid.as_str()), ("speakers", "mic"));
    assert_eq!(
        (out.config, inp.config),
        (
            IoConfig {
                input: false,
                output: true
            },
            IoConfig {
                input: true,
                output: false
            }
        )
    );
    // Each side counts from its own first cycle: a gap on one is not a gap on the other.
    inp.cycle(Some(1_000_512.0 + 512.0), 512, 0, &[vec![0.0; 512]]);
    out.cycle(Some(1_000_512.0), 512, 0, &[]);
    let blocks = drain(&mut st);
    assert_eq!(
        (blocks[1].0, blocks[1].2),
        (1024, BlockFlags::DISCONTINUITY)
    );
    let mut ticks = Vec::new();
    while let Some(t) = st.pop_output_tick() {
        ticks.push((t.start_sample, t.flags));
    }
    assert_eq!(ticks[1], (512, BlockFlags::NONE));
}

#[test]
fn default_output_follows_the_input_device_when_it_plays() {
    let s = Scripted::new(vec![blackhole(), speakers()]);
    let mut r = request(vec![0], 2);
    r.input_device = id("BlackHole2ch_UID");
    let st = backend(&s).open(r).expect("open");
    assert_eq!(
        st.negotiated().output_device.0,
        "coreaudio:BlackHole2ch_UID"
    );
    assert_eq!(s.runs.lock().expect("lock").len(), 1);
}

#[test]
fn selectors_and_probe() {
    let s = Scripted::new(vec![blackhole()]);
    let b = backend(&s);
    let mut r = request(vec![0], 0);
    r.input_device = DeviceSelector::Id(DeviceId("BlackHole2ch_UID".into()));
    assert!(
        matches!(b.open(r), Err(AudioError::DeviceNotFound { .. })),
        "the prefix is required"
    );
    let mut r = request(vec![2], 0);
    r.input_device = id("BlackHole2ch_UID");
    assert!(matches!(
        b.open(r),
        Err(AudioError::Unsupported(Unsupported::InputChannel {
            channel: 2,
            available: 2
        }))
    ));
    let mut r = request(vec![0], 3);
    r.input_device = id("BlackHole2ch_UID");
    assert!(matches!(
        b.open(r),
        Err(AudioError::Unsupported(Unsupported::OutputChannels {
            requested: 3,
            available: 2
        }))
    ));
    assert_eq!(
        b.probe(&id("BlackHole2ch_UID")),
        Presence::Present { generation: 77 }
    );
    assert_eq!(
        b.probe(&DeviceSelector::Default),
        Presence::Present { generation: 77 }
    );
    assert_eq!(b.probe(&id("gone")), Presence::Absent);
    s.set_device("BlackHole2ch_UID", |d| d.object_id = 78);
    assert_eq!(
        b.probe(&id("BlackHole2ch_UID")),
        Presence::Present { generation: 78 },
        "a replugged device's new object id is a new generation"
    );
    assert!(matches!(
        b.control_panel(&DeviceSelector::Default),
        Err(AudioError::Unsupported(Unsupported::ControlPanel {
            backend: BackendKind::CoreAudio
        }))
    ));
}

#[test]
fn a_device_hogged_by_another_process_is_unavailable() {
    let s = Scripted::new(vec![blackhole()]);
    s.set_device("BlackHole2ch_UID", |d| d.hog_pid = Some(1));
    let e = backend(&s).open(request(vec![0], 2)).expect_err("hogged");
    assert!(e.to_string().contains("exclusive use"), "{e}");
    s.set_device("BlackHole2ch_UID", |d| {
        d.hog_pid = Some(std::process::id() as i32)
    });
    backend(&s)
        .open(request(vec![0], 2))
        .expect("our own hog is fine");
}

#[test]
fn a_non_float_stream_is_refused_by_direction() {
    let s = Scripted::new(vec![blackhole()]);
    s.set_device("BlackHole2ch_UID", |d| {
        d.input.streams[0].format = StreamFormat::Other {
            format_id: 1,
            flags: 0,
            bits: 16,
        }
    });
    assert!(matches!(
        backend(&s).open(request(vec![0], 2)),
        Err(AudioError::Unsupported(Unsupported::SampleFormat { .. }))
    ));
}
