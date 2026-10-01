//! Spike CLI.
//!
//! ```text
//! audio-duplex --list [--backend cpal|jack|fake|all] [--host alsa|coreaudio|wasapi]
//! audio-duplex --run <seconds> [--backend cpal|jack|fake] [--host NAME]
//!              [--in-dev ID] [--out-dev ID] [--inputs 0,1] [--outputs N]
//!              [--rate HZ] [--buffer FRAMES] [--connect none|inputs|both]
//!              [--emit <level-dbfs>]   # capped at -20 dBFS; silence without it
//!              [--trace]               # print flagged/irregular blocks to stderr
//! ```
//! Output is JSON on stdout.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;

use spike_audio_duplex::backend::{AudioBackend, ConnectPolicy, DuplexRequest};
use spike_audio_duplex::cpal_backend::CpalBackend;
use spike_audio_duplex::fake::FakeBackend;
use spike_audio_duplex::output::{EmitLevel, OutputMode};
use spike_audio_duplex::stats::RunStats;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Which {
    Cpal,
    Jack,
    Fake,
}

impl Which {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "cpal" => Ok(Self::Cpal),
            "jack" => Ok(Self::Jack),
            "fake" => Ok(Self::Fake),
            other => Err(format!("unknown backend {other:?} (cpal|jack|fake)")),
        }
    }
}

#[derive(Debug)]
enum Mode {
    List(Vec<Which>),
    Run(Which, f64),
}

#[derive(Debug)]
struct Args {
    mode: Mode,
    trace: bool,
    host: Option<String>,
    req: DuplexRequest,
}

fn parse_args() -> Result<Args, String> {
    let mut list = false;
    let mut run: Option<f64> = None;
    let mut backend: Option<String> = None;
    let mut host = None;
    let mut trace = false;
    let mut req = DuplexRequest::default();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--list" => list = true,
            "--trace" => trace = true,
            "--run" => {
                run = Some(
                    val()?
                        .parse()
                        .map_err(|_| "--run takes seconds".to_string())?,
                )
            }
            "--backend" => backend = Some(val()?),
            "--host" => host = Some(val()?),
            "--in-dev" => req.input_device = Some(val()?),
            "--out-dev" => req.output_device = Some(val()?),
            "--inputs" => {
                req.input_map = val()?
                    .split(',')
                    .map(|c| c.trim().parse::<u16>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| "--inputs takes channel indices like 0,1".to_string())?
            }
            "--outputs" => {
                req.output_channels = val()?.parse().map_err(|_| "--outputs takes a count")?
            }
            "--rate" => req.sample_rate = Some(val()?.parse().map_err(|_| "--rate takes Hz")?),
            "--buffer" => {
                req.buffer_frames = Some(val()?.parse().map_err(|_| "--buffer takes frames")?)
            }
            "--connect" => {
                req.connect = match val()?.as_str() {
                    "none" => ConnectPolicy::None,
                    "inputs" => ConnectPolicy::InputsOnly,
                    "both" => ConnectPolicy::Both,
                    o => return Err(format!("--connect {o:?}: none|inputs|both")),
                }
            }
            "--emit" => {
                let level = EmitLevel::parse(&val()?).map_err(|e| e.to_string())?;
                req.output = OutputMode::Tone {
                    level,
                    freq_hz: 1000.0,
                };
            }
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let mode = match (list, run) {
        (true, None) => Mode::List(match backend.as_deref() {
            None | Some("all") => vec![Which::Cpal, Which::Jack, Which::Fake],
            Some(b) => vec![Which::parse(b)?],
        }),
        (false, Some(secs)) => Mode::Run(Which::parse(backend.as_deref().unwrap_or("cpal"))?, secs),
        _ => return Err("give exactly one of --list or --run <seconds>".into()),
    };
    Ok(Args {
        mode,
        trace,
        host,
        req,
    })
}

fn backend(which: Which, host: &Option<String>) -> Result<Box<dyn AudioBackend>, String> {
    match which {
        Which::Cpal => Ok(Box::new(CpalBackend { host: host.clone() })),
        Which::Fake => Ok(Box::new(FakeBackend::default())),
        #[cfg(all(feature = "jack", target_os = "linux"))]
        Which::Jack => Ok(Box::new(
            spike_audio_duplex::jack_backend::JackBackend::default(),
        )),
        #[cfg(not(all(feature = "jack", target_os = "linux")))]
        Which::Jack => Err("built without the `jack` feature".into()),
    }
}

#[derive(Default, Serialize)]
struct Span {
    n: u64,
    min_us: f64,
    max_us: f64,
    mean_us: f64,
}

impl Span {
    fn add(&mut self, v: f64) {
        if self.n == 0 {
            self.min_us = v;
            self.max_us = v;
        }
        self.n += 1;
        self.min_us = self.min_us.min(v);
        self.max_us = self.max_us.max(v);
        self.mean_us += (v - self.mean_us) / self.n as f64;
    }
}

fn run(
    which: Which,
    secs: f64,
    host: &Option<String>,
    req: &DuplexRequest,
    trace: bool,
) -> Result<(), String> {
    if let OutputMode::Tone { level, .. } = req.output {
        eprintln!(
            "WARNING: --emit given: a {:.1} dBFS 1 kHz tone will play on output channel 1",
            level.dbfs()
        );
    }
    let be = backend(which, host)?;
    let mut stream = be.open_duplex(req).map_err(|e| e.to_string())?;
    let mut stats = RunStats::new(stream.negotiated.sample_rate);
    let mut out_interval = Span::default();
    let mut out_lead = Span::default();
    let mut last_out_cb: Option<u64> = None;
    let mut buf = Vec::new();
    let mut last_in_cb: Option<u64> = None;
    // Per block-channel peak, so channel routing can be checked by ear-free inspection.
    let mut peaks = vec![0.0f32; usize::from(stream.capture.channels())];
    let t0 = Instant::now();
    let end = t0 + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        let mut idle = true;
        while let Some(h) = stream.capture.pop_into(&mut buf) {
            if trace {
                // Print anomalies with their timing so flag placement can be inspected.
                let dt = last_in_cb.map(|p| (h.callback_ns as f64 - p as f64) / 1e3);
                let nominal = f64::from(h.frames) / f64::from(stream.negotiated.sample_rate) * 1e6;
                let odd = dt.is_some_and(|d| (d - nominal).abs() > nominal * 0.5);
                if !h.flags.is_empty() || odd {
                    eprintln!(
                        "block start={} frames={} flags={:#x} dt_us={:?}",
                        h.start_sample,
                        h.frames,
                        h.flags.bits(),
                        dt.map(|d| d.round())
                    );
                }
            }
            last_in_cb = Some(h.callback_ns);
            stats.add(&h);
            let ch = usize::from(h.channels.max(1));
            for (i, v) in buf.iter().enumerate() {
                peaks[i % ch] = peaks[i % ch].max(v.abs());
            }
            idle = false;
        }
        while let Ok(t) = stream.output_ticks.pop() {
            if let Some(prev) = last_out_cb {
                out_interval.add((t.callback_ns as f64 - prev as f64) / 1e3);
            }
            last_out_cb = Some(t.callback_ns);
            if let Some(pb) = t.playback_ns {
                out_lead.add((pb as f64 - t.callback_ns as f64) / 1e3);
            }
            idle = false;
        }
        if idle {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let events = stream.events.snapshot();
    let transport = stream.capture.counters();
    let dropped = transport
        .blocks_dropped
        .load(std::sync::atomic::Ordering::Relaxed);
    let report = json!({
        "backend": which,
        "seconds": secs,
        "negotiated": stream.negotiated,
        "capture": stats.report(),
        "transport_blocks_dropped": dropped,
        "backend_events": events,
        "channel_peak_dbfs": peaks
            .iter()
            .map(|&p| if p > 0.0 { (20.0 * f64::from(p).log10() * 10.0).round() / 10.0 } else { f64::NEG_INFINITY })
            .map(|d| if d.is_finite() { json!(d) } else { json!("-inf") })
            .collect::<Vec<_>>(),
        "output_callback_interval": out_interval,
        "output_playback_lead": out_lead,
    });
    stream.stop();
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}");
            }
            eprintln!(
                "usage: audio-duplex --list [--backend cpal|jack|fake|all] [--host NAME]\n       \
                 audio-duplex --run <seconds> [--backend cpal|jack|fake] [--host NAME] \
                 [--in-dev ID] [--out-dev ID] [--inputs 0,1] [--outputs N] [--rate HZ] \
                 [--buffer FRAMES] [--connect none|inputs|both] [--emit <level-dbfs>] [--trace]"
            );
            return ExitCode::from(2);
        }
    };
    match args.mode {
        Mode::List(which) => {
            let entries: Vec<_> = which
                .into_iter()
                .map(|w| {
                    match backend(w, &args.host)
                        .and_then(|b| b.enumerate().map_err(|e| e.to_string()))
                    {
                        Ok(devs) => json!({ "backend": w, "devices": devs }),
                        Err(e) => json!({ "backend": w, "error": e }),
                    }
                })
                .collect();
            match serde_json::to_string_pretty(&entries) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Mode::Run(which, secs) => match run(which, secs, &args.host, &args.req, args.trace) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}
