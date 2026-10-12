//! cpal's Core Audio timestamps on one device, beside the host clock read in each callback,
//! and (with `click`) a click loopback timed by those timestamps. On BlackHole a sample
//! written at output sample time T is read at input sample time T (hal-timestamps shows
//! it), so a click's capture instant minus its playback instant is cpal's timestamp error.
//!
//!   cpal-ts "BlackHole 2ch" <seconds> <buffer frames> [click]
//!
//! Without `click` the output plays silence. `click` (a -40 dBFS one-sample click every
//! 24000 frames) runs only into BlackHole on a GitHub-hosted runner.
#![allow(deprecated)] // libc's mach time functions point at the mach2 crate; same calls.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const RATE: u32 = 48_000;

#[cfg(not(target_os = "macos"))]
fn now_ns() -> u64 {
    // Elsewhere only so the probe type-checks; its numbers mean something on macOS only.
    0
}

#[cfg(target_os = "macos")]
fn now_ns() -> u64 {
    let mut tb = libc::mach_timebase_info { numer: 0, denom: 0 };
    unsafe { libc::mach_timebase_info(&mut tb) };
    let t = unsafe { libc::mach_absolute_time() };
    (t as u128 * tb.numer as u128 / tb.denom as u128) as u64
}

fn frame_ns(f: usize) -> u64 {
    (f as f64 * 1e9 / RATE as f64).round() as u64
}

#[derive(Clone, Copy)]
struct Row {
    /// Host clock when the callback ran.
    now: u64,
    /// cpal's `callback` instant.
    callback: u64,
    /// cpal's `capture` (input) or `playback` (output) instant.
    stamp: u64,
    frames: usize,
}

fn spread(label: &str, v: impl Iterator<Item = f64>) {
    let v: Vec<f64> = v.collect();
    if v.is_empty() {
        println!("{label}: none");
        return;
    }
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let min = v.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    println!("{label}: mean {mean:.3} ms, min {min:.3}, max {max:.3} (n {})", v.len());
}

fn ms(a: u64, b: u64) -> f64 {
    (a as f64 - b as f64) / 1e6
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let want = args.get(1).cloned().unwrap_or_else(|| "BlackHole 2ch".into());
    let secs: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(6.0);
    let buf: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let click = args.get(4).is_some_and(|s| s == "click");
    if click
        && (std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
            || !want.starts_with("BlackHole"))
    {
        eprintln!("click plays only into BlackHole on a GitHub-hosted runner");
        std::process::exit(2);
    }

    let host = cpal::default_host();
    let dev = host
        .devices()
        .expect("devices")
        .find(|d| d.description().is_ok_and(|x| x.name().to_string() == want))
        .unwrap_or_else(|| panic!("no device named {want}"));
    let cfg = cpal::StreamConfig {
        channels: 2,
        sample_rate: RATE,
        buffer_size: cpal::BufferSize::Fixed(buf),
    };

    let ins = Arc::new(Mutex::new(Vec::<Row>::with_capacity(8192)));
    let outs = Arc::new(Mutex::new(Vec::<Row>::with_capacity(8192)));
    let clicks_out = Arc::new(Mutex::new(Vec::<u64>::new()));
    let clicks_in = Arc::new(Mutex::new(Vec::<u64>::new()));

    let (o, co) = (Arc::clone(&outs), Arc::clone(&clicks_out));
    let mut written: u64 = 0;
    let mut next_click: u64 = 4800;
    let out_stream = dev
        .build_output_stream::<f32, _, _>(
            cfg.clone(),
            move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                let now = now_ns();
                let ts = info.timestamp();
                let playback = ts.playback.as_nanos() as u64;
                let frames = data.len() / 2;
                data.fill(0.0);
                if click && next_click < written + frames as u64 {
                    let f = (next_click - written) as usize;
                    data[2 * f] = 0.01;
                    data[2 * f + 1] = 0.01;
                    co.lock().unwrap().push(playback + frame_ns(f));
                    next_click += 24_000;
                }
                written += frames as u64;
                o.lock().unwrap().push(Row {
                    now,
                    callback: ts.callback.as_nanos() as u64,
                    stamp: playback,
                    frames,
                });
            },
            |e| eprintln!("output error: {e}"),
            None,
        )
        .expect("output stream");

    let (i, ci) = (Arc::clone(&ins), Arc::clone(&clicks_in));
    let mut last: Option<u64> = None;
    let in_stream = dev
        .build_input_stream::<f32, _, _>(
            cfg.clone(),
            move |data: &[f32], info: &cpal::InputCallbackInfo| {
                let now = now_ns();
                let ts = info.timestamp();
                let capture = ts.capture.as_nanos() as u64;
                let frames = data.len() / 2;
                for f in 0..frames {
                    if data[2 * f].abs() > 0.005 {
                        let t = capture + frame_ns(f);
                        if last.is_none_or(|l| t > l + 100_000_000) {
                            ci.lock().unwrap().push(t);
                            last = Some(t);
                        }
                    }
                }
                i.lock().unwrap().push(Row {
                    now,
                    callback: ts.callback.as_nanos() as u64,
                    stamp: capture,
                    frames,
                });
            },
            |e| eprintln!("input error: {e}"),
            None,
        )
        .expect("input stream");

    in_stream.play().expect("input play");
    out_stream.play().expect("output play");
    std::thread::sleep(Duration::from_secs_f64(secs));
    drop(in_stream);
    drop(out_stream);

    let buf_ms = buf as f64 * 1e3 / RATE as f64;
    println!("buffer {buf} frames = {buf_ms:.3} ms");
    for (side, rows, stamp) in [("input", &ins, "capture"), ("output", &outs, "playback")] {
        let rows = rows.lock().unwrap();
        // The first callbacks run while the device starts; skip them.
        let r = &rows[rows.len().min(4)..];
        println!("{side}: {} callbacks, frames {:?}", rows.len(), r.first().map(|x| x.frames));
        spread(&format!("  {side} now - callback"), r.iter().map(|x| ms(x.now, x.callback)));
        spread(&format!("  {side} {stamp} - callback"), r.iter().map(|x| ms(x.stamp, x.callback)));
        spread(
            &format!("  {side} callback interval"),
            r.windows(2).map(|w| ms(w[1].callback, w[0].callback)),
        );
        spread(&format!("  {side} now interval"), r.windows(2).map(|w| ms(w[1].now, w[0].now)));
        for x in r.iter().take(4) {
            println!("    now {} callback {} {stamp} {} frames {}", x.now, x.callback, x.stamp, x.frames);
        }
    }
    // Same callback cycle: input and output callbacks of one IOProc share a wall time.
    {
        let (ri, ro) = (ins.lock().unwrap(), outs.lock().unwrap());
        if let (Some(a), Some(b)) = (ri.get(10), ro.get(10)) {
            println!(
                "output callback - input callback (10th each): {:.3} ms; output now - input now: {:.3} ms",
                ms(b.callback, a.callback),
                ms(b.now, a.now)
            );
        }
    }
    if click {
        let (co, ci) = (clicks_out.lock().unwrap(), clicks_in.lock().unwrap());
        println!("clicks out {}, back {}", co.len(), ci.len());
        let d: Vec<f64> = co.iter().zip(ci.iter()).map(|(o, i)| ms(*i, *o)).collect();
        for (k, x) in d.iter().enumerate() {
            println!("  click {k}: capture - playback {x:.3} ms = {:.1} frames", x * RATE as f64 / 1e3);
        }
        spread("click capture - playback", d.into_iter());
    }
}
