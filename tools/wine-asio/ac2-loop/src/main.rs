//! Loopback check of ac2's ASIO backend under Wine: opens WineASIO through `AsioBackend`
//! `CYCLES` times (default 20), each time with a fresh driver load, `start()` and stream
//! origin. Output 1 carries a -20 dBFS single-sample impulse every `PERIOD` samples; run.sh
//! connects `ac2:out_1 -> ac2:in_1` inside the JACK dummy graph. Per impulse found on input 1
//! the delay is `input index - output index` on the stream's shared sample index, which is
//! the same in every cycle only when input and output really share one exact index.
//!
//! `MODE=bufsize`: silent instead. Opens WineASIO, prints `READY` and waits for the stream to
//! end (run.sh then changes JACK's buffer size, and wineasio asks for a reset), then opens it
//! again and checks the new buffer size and an unbroken index, as the daemon's recovery would.
//!
//! Refuses any driver other than WineASIO: on a real interface this would put impulses on a
//! loudspeaker.

#[cfg(not(windows))]
fn main() {
    eprintln!("ac2-loop only runs on Windows (under Wine)");
}

#[cfg(windows)]
fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("ac2-loop failed: {e}");
            std::process::exit(2);
        }
    }
}

#[cfg(windows)]
const PERIOD: u64 = 4800;
#[cfg(windows)]
const PEAK: f32 = 0.1;

#[cfg(windows)]
struct Pulses {
    n: u64,
}

#[cfg(windows)]
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

#[cfg(windows)]
fn run() -> Result<bool, Box<dyn std::error::Error>> {
    if std::env::var("MODE").as_deref() == Ok("bufsize") {
        return bufsize();
    }
    loopback()
}

/// Silent: ends on the driver's reset request, then reopens at the new size.
#[cfg(windows)]
fn bufsize() -> Result<bool, Box<dyn std::error::Error>> {
    use std::io::Write;
    use std::time::{Duration, Instant};

    use ac2_audio::{Backend, BlockFlags, DeviceSelector, DuplexRequest, MaxLevel};

    let backend = ac2_audio::AsioBackend::new();
    let open = |b: &ac2_audio::AsioBackend| -> Result<ac2_audio::DuplexStream, Box<dyn std::error::Error>> {
        let mut req = DuplexRequest::new(vec![0, 1], 2, MaxLevel::from_peak_db(-20.0)?);
        req.input_device = DeviceSelector::Id(ac2_audio::DeviceId("WineASIO Driver".into()));
        req.output_device = req.input_device.clone();
        Ok(b.open(req)?)
    };
    // Drains capture; returns (blocks, index breaks other than FIRST).
    let drain = |s: &mut ac2_audio::DuplexStream, next: &mut Option<u64>| {
        let mut blocks = 0u64;
        let mut breaks = 0u64;
        while let Some(()) = s.capture().pop_with(|h, _, _| {
            if next.is_some_and(|n| n != h.start_sample)
                || h.flags.bits() & !BlockFlags::FIRST.bits() != 0
            {
                breaks += 1;
            }
            *next = Some(h.start_sample + u64::from(h.frames));
            blocks += 1;
        }) {}
        (blocks, breaks)
    };

    let mut s = open(&backend)?;
    let before = s.negotiated().buffer_frames;
    println!("opened, buffer {before:?}");
    println!("READY");
    std::io::stdout().flush()?;
    let mut next = None;
    let t0 = Instant::now();
    let mut ended_after = None;
    while t0.elapsed() < Duration::from_secs(30) {
        drain(&mut s, &mut next);
        if s.events().ended {
            ended_after = Some(t0.elapsed());
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("ended: {ended_after:?}, events {:?}", s.events());
    let _ = s.stop(Duration::from_secs(2));
    let Some(_) = ended_after else {
        println!("FAIL: the stream did not end");
        return Ok(false);
    };

    let t1 = Instant::now();
    let mut s = open(&backend)?;
    let after = s.negotiated().buffer_frames;
    println!(
        "reopened in {:?}, buffer {after:?}, index {:?}",
        t1.elapsed(),
        s.negotiated().index
    );
    let mut next = None;
    let (mut blocks, mut breaks) = (0, 0);
    let t2 = Instant::now();
    while t2.elapsed() < Duration::from_secs(5) {
        let (b, k) = drain(&mut s, &mut next);
        blocks += b;
        breaks += k;
        std::thread::sleep(Duration::from_millis(20));
    }
    let events = s.events();
    let _ = s.stop(Duration::from_secs(2));
    println!("after reopen: {blocks} blocks, {breaks} breaks, events {events:?}");
    println!("# summary");
    let ok = after != before && blocks > 0 && breaks == 0 && !events.ended;
    println!(
        "{}",
        if ok {
            "PASS: ended, reopened at the new size"
        } else {
            "FAIL"
        }
    );
    Ok(ok)
}

#[cfg(windows)]
fn loopback() -> Result<bool, Box<dyn std::error::Error>> {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    use ac2_audio::{
        Backend, BlockFlags, DeviceSelector, DuplexRequest, Gain, HistoryRequest, MaxLevel,
        OutputSource,
    };

    let cycles: u32 = std::env::var("CYCLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let seconds: f64 = std::env::var("SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3.0);
    let backend = ac2_audio::AsioBackend::new();
    let devices = backend.enumerate()?;
    let dev = devices
        .iter()
        .find(|d| d.id.0 == "WineASIO Driver")
        .ok_or_else(|| {
            let ids: Vec<&str> = devices.iter().map(|d| d.id.0.as_str()).collect();
            format!("no WineASIO driver among {ids:?}: refusing to emit on anything else")
        })?;
    println!("device {} ({:?})", dev.id.0, dev.notes);
    for (dir, caps) in [("in", &dev.input), ("out", &dev.output)] {
        if let Some(c) = caps {
            println!("  {dir}: {:?}", c.channel_names);
        }
    }

    let mut delays: BTreeMap<i64, u32> = BTreeMap::new();
    let mut ok = true;
    for cycle in 0..cycles {
        let (mut gen_handle, port) = ac2_audio::generator(vec![0u16])?;
        let mut req = DuplexRequest::new(vec![0, 1], 2, MaxLevel::from_peak_db(-20.0)?);
        req.input_device = DeviceSelector::Id(dev.id.clone());
        req.output_device = DeviceSelector::Id(dev.id.clone());
        req.output = OutputSource::Generator(port);
        req.history = Some(HistoryRequest {
            channel: 0,
            seconds: seconds + 2.0,
        });
        let mut stream = backend.open(req)?;
        let neg = format!("{:?}", stream.negotiated());
        gen_handle.set_gain(Gain::UNITY);
        if gen_handle.set_source(Box::new(Pulses { n: 0 })).is_err() {
            return Err("generator queue full".into());
        }
        gen_handle.start();

        let mut inputs = Vec::new();
        let mut flags = Vec::new();
        let mut blocks = 0u64;
        let mut next = None;
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs_f64(seconds) {
            while let Some(()) = stream.capture().pop_with(|h, a, b| {
                let ch = usize::from(h.channels);
                let s = if b.is_empty() {
                    a.to_vec()
                } else {
                    [a, b].concat()
                };
                for (i, f) in s.chunks(ch).enumerate() {
                    if f[0] > PEAK / 2.0 {
                        inputs.push(h.start_sample + i as u64);
                    }
                }
                if next.is_some_and(|n| n != h.start_sample)
                    || h.flags.bits() & !BlockFlags::FIRST.bits() != 0
                {
                    flags.push((h.start_sample, h.flags.bits()));
                }
                next = Some(h.start_sample + u64::from(h.frames));
                blocks += 1;
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
                .filter(|(_, v)| **v > PEAK / 2.0)
                .map(|(i, _)| start + i as u64)
                .collect();
        }
        let events = stream.events();
        let outcome = stream.stop(Duration::from_secs(2));
        let mut cycle_delays: BTreeMap<i64, u32> = BTreeMap::new();
        for &j in &inputs {
            if let Some(&k) = outputs.iter().rev().find(|&&k| k <= j) {
                *cycle_delays.entry(j as i64 - k as i64).or_default() += 1;
            }
        }
        for (d, n) in &cycle_delays {
            *delays.entry(*d).or_default() += n;
        }
        if !flags.is_empty() {
            ok = false;
        }
        println!(
            "cycle {cycle:2}: {blocks} blocks, {} out / {} in impulses, delays {cycle_delays:?}, \
             breaks {flags:?}, events {events:?}, stop {outcome:?}",
            outputs.len(),
            inputs.len()
        );
        if cycle == 0 {
            println!("negotiated {neg}");
        }
        if cycle_delays.is_empty() {
            ok = false;
        }
    }
    println!("# summary");
    println!("cycles {cycles}, delay histogram (samples: impulses) {delays:?}");
    if delays.len() != 1 {
        ok = false;
    }
    println!(
        "{}",
        if ok {
            "PASS: one constant delay"
        } else {
            "FAIL"
        }
    );
    Ok(ok)
}
