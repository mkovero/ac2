//! Exact-index check of ac2's Core Audio backend on BlackHole 2ch, BlackHole's own loop from
//! output 1 to input 1.
//!
//! `coreaudio-loop loop`: opens BlackHole through `CoreAudioBackend` `CYCLES` times (default
//! 20) at each buffer size in `BUFFERS` (default `2048,1024`), each a fresh IOProc and
//! stream origin. Output 1 carries a -20 dBFS single-sample impulse every `PERIOD` samples.
//! Per impulse found on input 1 the delay is `input index - output index` on the stream's
//! shared sample index. It passes when every delay equals the stated latency
//! (`StaticLatency::Frames`, input + output) and no other sample on either input is
//! non-zero: BlackHole is a bit-exact loop, so anything else is the backend's doing.
//! A round trip with a block flagged as a break inside it (the VM overloaded the HAL's I/O
//! thread: `XRUN | DISCONTINUITY`) is reported apart and not judged: across a flagged break
//! the stream itself says the line is not continuous.
//!
//! `coreaudio-loop consent`: silent. The microphone authorization, a bounded request for it
//! (does the call return, does its handler run), then a silent open with a short consent
//! wait, which must return rather than hang whatever macOS answers.
//!
//! Refuses anything but BlackHole on a GitHub-hosted runner (the same guards as the sox
//! stimulus in macos-blackhole.sh): on a real interface this would put impulses on a
//! loudspeaker.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("coreaudio-loop only runs on macOS");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    let r = match std::env::args().nth(1).as_deref() {
        Some("loop") => mac::loopback(),
        Some("consent") => mac::consent(),
        _ => {
            eprintln!("usage: coreaudio-loop loop|consent");
            std::process::exit(2);
        }
    };
    match r {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("coreaudio-loop failed: {e}");
            std::process::exit(2);
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::time::{Duration, Instant};

    use ac2_audio::{
        Backend, BlockFlags, CoreAudioBackend, DeviceId, DeviceSelector, DuplexRequest, Gain,
        HistoryRequest, IndexExactness, MaxLevel, OutputSource, Presence, StaticLatency,
    };

    type Res<T> = Result<T, Box<dyn Error>>;

    const UID: &str = "BlackHole2ch_UID";
    /// Long enough apart that the generator's fade-in (20 ms) is over before the first one.
    const PERIOD: u64 = 4800;
    /// -20 dBFS, the request's maximum level.
    const PEAK: f32 = 0.1;

    struct Pulses {
        n: u64,
    }

    impl ac2_audio::SignalSource for Pulses {
        fn fill(&mut self, out: &mut [f32]) {
            for s in out {
                *s = if self.n.is_multiple_of(PERIOD) {
                    PEAK
                } else {
                    0.0
                };
                self.n += 1;
            }
        }
    }

    fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    /// The guards: a GitHub-hosted runner, and BlackHole as Core Audio lists it.
    fn guard() -> Res<DeviceId> {
        if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
            || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
        {
            return Err("plays only on a GitHub-hosted runner".into());
        }
        let d = cahal::System::device_by_uid(UID)?.ok_or("no BlackHole 2ch device")?;
        let info = d.info()?;
        if !info.name.starts_with("BlackHole") || info.transport != cahal::Transport::Virtual {
            return Err(format!(
                "{UID} is '{}' ({:?}), not the virtual BlackHole: refusing",
                info.name, info.transport
            )
            .into());
        }
        println!(
            "device {} '{}' ({:?}), object id {}",
            UID,
            info.name,
            info.transport,
            d.object_id()
        );
        Ok(DeviceId(format!("coreaudio:{UID}")))
    }

    fn generation(b: &CoreAudioBackend, id: &DeviceId) -> Option<u64> {
        match b.probe(&DeviceSelector::Id(id.clone())) {
            Presence::Present { generation } => Some(generation),
            _ => None,
        }
    }

    pub(crate) fn loopback() -> Res<bool> {
        let id = guard()?;
        let cycles: u32 = env("CYCLES", 20);
        let seconds: f64 = env("SECONDS", 2.0);
        let buffers: Vec<u32> = std::env::var("BUFFERS")
            .unwrap_or_else(|_| "2048,1024".into())
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        let backend = CoreAudioBackend::new().with_consent_wait(Duration::from_secs(5));
        let caps = backend
            .enumerate()?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or("BlackHole is not in the Core Audio backend's list")?;
        println!(
            "listed: '{}', clock {:?}, index {:?}, latency {:?}, notes {:?}",
            caps.name, caps.duplex_clock, caps.index, caps.latency, caps.notes
        );
        let mut ok = true;
        let mut generations = Vec::new();
        for &buffer in &buffers {
            let mut delays: BTreeMap<i64, u32> = BTreeMap::new();
            let mut across: BTreeMap<i64, u32> = BTreeMap::new();
            let mut stated_all: BTreeMap<u32, u32> = BTreeMap::new();
            let mut buffer_ok = true;
            for cycle in 0..cycles {
                generations.push(generation(&backend, &id));
                let r = one_cycle(&backend, &id, buffer, seconds)?;
                *stated_all.entry(r.stated).or_default() += 1;
                for (d, n) in &r.delays {
                    *delays.entry(*d).or_default() += n;
                }
                for (d, n) in &r.across {
                    *across.entry(*d).or_default() += n;
                }
                let good = r.delays.len() == 1
                    && r.delays.contains_key(&i64::from(r.stated))
                    && r.strays == 0
                    && r.exact;
                println!(
                    "{buffer} cycle {cycle:2}: {} Hz, buffer {:?}, stated {} (in {} + out {}), \
                     {} out / {} in impulses, delays {:?}, across a break {:?}, strays {}, \
                     breaks {:?}, events {}, {}",
                    r.rate,
                    r.buffer,
                    r.stated,
                    r.stated_in,
                    r.stated_out,
                    r.outputs,
                    r.inputs,
                    r.delays,
                    r.across,
                    r.strays,
                    r.breaks,
                    r.events,
                    if good { "ok" } else { "MISMATCH" }
                );
                buffer_ok &= good;
            }
            println!(
                "RESULT buffer {buffer}: stated {stated_all:?}, measured {delays:?}, across a flagged \
                 break {across:?}, {}",
                if buffer_ok { "exact" } else { "FAIL" }
            );
            ok &= buffer_ok;
        }
        generations.push(generation(&backend, &id));
        generations.dedup();
        println!("probe generations (object ids) across every open/close: {generations:?}");
        println!(
            "{}",
            if ok {
                "PASS: every impulse back at T + stated latency"
            } else {
                "FAIL"
            }
        );
        Ok(ok)
    }

    struct CycleResult {
        rate: u32,
        buffer: Option<u32>,
        stated: u32,
        stated_in: u32,
        stated_out: u32,
        exact: bool,
        outputs: usize,
        inputs: usize,
        delays: BTreeMap<i64, u32>,
        /// Round trips across a flagged break, kept apart.
        across: BTreeMap<i64, u32>,
        strays: u64,
        breaks: Vec<(u64, u32)>,
        events: String,
    }

    fn one_cycle(
        b: &CoreAudioBackend,
        id: &DeviceId,
        buffer: u32,
        seconds: f64,
    ) -> Res<CycleResult> {
        let (mut gen_handle, port) = ac2_audio::generator(vec![0u16])?;
        let mut req = DuplexRequest::new(vec![0, 1], 2, MaxLevel::from_peak_db(-20.0)?);
        req.input_device = DeviceSelector::Id(id.clone());
        req.output_device = DeviceSelector::Id(id.clone());
        req.buffer_frames = Some(buffer);
        req.output = OutputSource::Generator(port);
        req.history = Some(HistoryRequest {
            channel: 0,
            seconds: seconds + 2.0,
        });
        let mut stream = b.open(req)?;
        let neg = stream.negotiated().clone();
        let (stated_in, stated_out) = match neg.latency {
            StaticLatency::Frames { input, output, .. } => (input, output),
            other => return Err(format!("stated latency {other:?}, expected frames").into()),
        };
        gen_handle.set_gain(Gain::UNITY);
        if gen_handle.set_source(Box::new(Pulses { n: 1 })).is_err() {
            return Err("generator queue full".into());
        }
        gen_handle.start();

        let mut inputs = Vec::new();
        let mut strays = 0u64;
        let mut breaks = Vec::new();
        let mut next = None;
        let t0 = Instant::now();
        // Only what arrived while the generator ran at full level: its fade-out on stop
        // would scale an impulse into a stray.
        while t0.elapsed() < Duration::from_secs_f64(seconds) {
            while let Some(()) = stream.capture().pop_with(|h, a, c| {
                let ch = usize::from(h.channels);
                let s = if c.is_empty() {
                    a.to_vec()
                } else {
                    [a, c].concat()
                };
                for (i, f) in s.chunks(ch).enumerate() {
                    if (f[0] - PEAK).abs() < 1e-4 {
                        inputs.push(h.start_sample + i as u64);
                    } else if f[0] != 0.0 {
                        strays += 1;
                    }
                    if f[1..].iter().any(|v| *v != 0.0) {
                        strays += 1;
                    }
                }
                if next.is_some_and(|n| n != h.start_sample)
                    || h.flags.bits() & !BlockFlags::FIRST.bits() != 0
                {
                    breaks.push((h.start_sample, h.flags.bits()));
                }
                next = Some(h.start_sample + u64::from(h.frames));
            }) {}
            std::thread::sleep(Duration::from_millis(20));
        }
        gen_handle.stop();
        let mut outputs = Vec::new();
        if let Some(h) = stream.history() {
            let end = h.written_end();
            let start = end.saturating_sub(h.capacity());
            let mut buf = vec![0.0f32; (end - start) as usize];
            h.read(start, &mut buf)?;
            outputs = buf
                .iter()
                .enumerate()
                .filter(|(_, v)| (**v - PEAK).abs() < 1e-4)
                .map(|(i, _)| start + i as u64)
                .collect();
        }
        let events = format!("{:?}", stream.events());
        let _ = stream.stop(Duration::from_secs(2));
        let mut delays: BTreeMap<i64, u32> = BTreeMap::new();
        let mut across: BTreeMap<i64, u32> = BTreeMap::new();
        for &j in &inputs {
            if let Some(&k) = outputs.iter().rev().find(|&&k| k <= j) {
                // A round trip with a flagged break (an overload: XRUN | DISCONTINUITY) inside
                // it is outside the exactness claim: the stream said the line broke there.
                let flagged = breaks.iter().any(|&(at, _)| at > k && at <= j);
                let into = if flagged { &mut across } else { &mut delays };
                *into.entry(j as i64 - k as i64).or_default() += 1;
            }
        }
        Ok(CycleResult {
            rate: neg.sample_rate,
            buffer: neg.buffer_frames,
            stated: stated_in + stated_out,
            stated_in,
            stated_out,
            exact: neg.index == IndexExactness::Exact,
            outputs: outputs.len(),
            inputs: inputs.len(),
            delays,
            across,
            strays,
            breaks,
            events,
        })
    }

    /// Silent. Reports what macOS does with a microphone request from this process, then
    /// that a silent open returns within its consent wait.
    pub(crate) fn consent() -> Res<bool> {
        let id = guard()?;
        let wait: u64 = env("WAIT", 10);
        println!("authorization before: {:?}", cahal::consent::microphone());
        let t = Instant::now();
        let r = cahal::consent::request_microphone(Duration::from_secs(wait));
        println!(
            "request (bounded at {wait} s): {:?} after {:.2} s; call returned {}, handler ran {}",
            r.authorization,
            t.elapsed().as_secs_f64(),
            r.call_returned,
            r.answered
        );
        println!("authorization after: {:?}", cahal::consent::microphone());

        let (tx, rx) = std::sync::mpsc::channel();
        let open_id = id.clone();
        std::thread::spawn(move || {
            let b = CoreAudioBackend::new().with_consent_wait(Duration::from_secs(5));
            let mut req =
                DuplexRequest::new(vec![0, 1], 2, MaxLevel::from_peak_db(-20.0).expect("level"));
            req.input_device = DeviceSelector::Id(open_id.clone());
            req.output_device = DeviceSelector::Id(open_id);
            let t = Instant::now();
            let r = b.open(req).map(|mut s| {
                std::thread::sleep(Duration::from_millis(500));
                let mut blocks = 0u64;
                while let Some(()) = s.capture().pop_with(|_, _, _| blocks += 1) {}
                let _ = s.stop(Duration::from_secs(2));
                blocks
            });
            let _ = tx.send((r.map_err(|e| e.to_string()), t.elapsed()));
        });
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok((Ok(blocks), t)) => println!(
                "silent open: ran, {blocks} blocks in 0.5 s (opened in {:.2} s)",
                t.as_secs_f64()
            ),
            Ok((Err(e), t)) => {
                println!("silent open: refused in {:.2} s: {e}", t.as_secs_f64())
            }
            Err(_) => {
                println!("silent open: HANG (no answer in 30 s)");
                return Ok(false);
            }
        }
        Ok(true)
    }
}
