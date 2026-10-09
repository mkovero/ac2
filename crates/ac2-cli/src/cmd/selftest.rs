//! `selftest duplex`: the in-process duplex check ([`crate::selftest`]) and its report.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ac2_audio::{
    Backend, ClockRelation, DeviceCaps, DeviceSelector, FakeBackend, FakeConfig, IndexExactness,
};

use crate::args::{BackendArg, Cli, SelftestCmd, SelftestDuplex};
use crate::output::Out;
use crate::selftest::{self, DuplexReport, DuplexSpec, Emit, SetupError, Spread};
use crate::watch::quit_signal;
use crate::{BUILD_ID, CliError};

pub(crate) async fn run(_cli: &Cli, cmd: &SelftestCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        SelftestCmd::Duplex(a) => duplex(a, out).await,
    }
}

/// The simulated device `--backend fake` tests: output 1 returns on input 1 through a
/// 32-sample cable, in real time.
fn fake() -> Result<FakeBackend, CliError> {
    use ac2_audio::fake::{FakeDrive, FakePath, Pace};
    FakeBackend::new(FakeConfig {
        drive: FakeDrive::Thread(Pace::Realtime),
        inputs: 2,
        outputs: 2,
        paths: vec![FakePath::loopback(0, 0, 32)],
        ..FakeConfig::default()
    })
    .map_err(|e| CliError::Usage(e.to_string()))
}

fn backend(b: BackendArg) -> Result<Arc<dyn Backend>, CliError> {
    match b {
        BackendArg::Fake => Ok(Arc::new(fake()?)),
        #[cfg(target_os = "linux")]
        BackendArg::Jack => Ok(Arc::new(ac2_audio::JackBackend::new(
            ac2_audio::JackConfig::default(),
        ))),
        #[cfg(target_os = "linux")]
        BackendArg::Cpal => Err(CliError::Usage(
            "Linux audio goes through JACK (JACK2, or PipeWire through pipewire-jack): use \
             --backend jack"
                .into(),
        )),
        #[cfg(not(target_os = "linux"))]
        BackendArg::Cpal => Ok(Arc::new(ac2_audio::CpalBackend::new())),
        #[cfg(not(target_os = "linux"))]
        BackendArg::Jack => Err(CliError::Usage(
            "JACK is supported on Linux only: use --backend cpal (the system audio)".into(),
        )),
    }
}

/// The listed device `name` (an id, or a name ignoring case) names.
fn find<'a>(devices: &'a [DeviceCaps], name: &str) -> Result<&'a DeviceCaps, CliError> {
    devices
        .iter()
        .find(|d| d.id.0 == name)
        .or_else(|| devices.iter().find(|d| d.name == name))
        .or_else(|| devices.iter().find(|d| d.name.eq_ignore_ascii_case(name)))
        .ok_or_else(|| {
            let known: Vec<String> = devices
                .iter()
                .map(|d| format!("{} ({})", d.id, d.name))
                .collect();
            CliError::Usage(format!(
                "no device {name:?}; listed: {}",
                if known.is_empty() {
                    "none".to_owned()
                } else {
                    known.join(", ")
                }
            ))
        })
}

/// Device selection and channel defaults from what the backend lists.
fn spec(a: &SelftestDuplex, devices: &[DeviceCaps]) -> Result<DuplexSpec, CliError> {
    let input = match &a.device {
        Some(n) => Some(find(devices, n)?),
        None => devices
            .iter()
            .find(|d| d.input.as_ref().is_some_and(|c| c.system_default))
            .or_else(|| devices.iter().find(|d| d.input.is_some())),
    };
    let output = match &a.out_device {
        Some(n) => Some(find(devices, n)?),
        None => input.filter(|d| d.output.is_some()).or_else(|| {
            devices
                .iter()
                .find(|d| d.output.as_ref().is_some_and(|c| c.system_default))
        }),
    };
    let input_map = match (&a.inputs, input.and_then(|d| d.input.as_ref())) {
        (Some(c), _) => c.0.clone(),
        (None, Some(caps)) if caps.max_channels > 0 => (0..caps.max_channels).collect(),
        _ => {
            return Err(CliError::Usage(
                "no input device found to default to: name the inputs with --in".into(),
            ));
        }
    };
    let output_channels = a.outputs.unwrap_or_else(|| {
        output
            .and_then(|d| d.output.as_ref())
            .map_or(0, |c| c.max_channels.min(2))
    });
    let selector = |name: &Option<String>, dev: Option<&DeviceCaps>| match (name, dev) {
        (Some(_), Some(d)) => DeviceSelector::Id(d.id.clone()),
        _ => DeviceSelector::Default,
    };
    let rate = match a.rate {
        Some(f) if f.0.0.fract() == 0.0 && f.0.0 > 0.0 && f.0.0 <= f64::from(u32::MAX) => {
            Some(f.0.0 as u32)
        }
        Some(f) => {
            return Err(CliError::Usage(format!(
                "--rate {} Hz: a whole number of hertz",
                f.0.0
            )));
        }
        None => None,
    };
    let buffer = match a.buffer {
        Some(b) => Some(
            u32::try_from(b.0)
                .ok()
                .filter(|&b| b > 0)
                .ok_or_else(|| CliError::Usage("--buffer: a positive number of samples".into()))?,
        ),
        None => None,
    };
    Ok(DuplexSpec {
        input_device: selector(&a.device, input),
        output_device: selector(&a.out_device, output),
        input_map,
        output_channels,
        sample_rate: rate,
        buffer_frames: buffer,
    })
}

async fn duplex(a: &SelftestDuplex, out: &mut Out<'_>) -> Result<(), CliError> {
    let backend = backend(a.backend)?;
    let devices = {
        let b = Arc::clone(&backend);
        tokio::task::spawn_blocking(move || b.enumerate())
            .await
            .map_err(|e| CliError::Io(std::io::Error::other(e)))?
            .map_err(|e| CliError::Usage(format!("cannot list devices: {e}")))?
    };
    let spec = spec(a, &devices)?;
    let emit = match (a.emit, a.loopback_out, a.loopback_in) {
        (Some(level), Some(o), Some(i)) => Some(Emit {
            level_dbfs: level.0.0,
            loopback_out: o.0,
            loopback_in: i.0,
        }),
        _ => None,
    };
    if let Some(e) = emit {
        selftest::check_level(e.level_dbfs).map_err(|e| CliError::Refused(e.to_string()))?;
    }
    let duration = Duration::from_secs_f64(a.duration.0.0.clamp(0.1, 86_400.0));
    match emit {
        Some(e) => eprintln!(
            "ac2: emitting pink noise at {:.1} dBFS on output {} for {:.0} s (Ctrl-C fades out)",
            e.level_dbfs,
            e.loopback_out + 1,
            duration.as_secs_f64()
        ),
        None => eprintln!(
            "ac2: running {:.0} s, outputs silent (Ctrl-C stops early)",
            duration.as_secs_f64()
        ),
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut task = {
        let (b, stop) = (Arc::clone(&backend), Arc::clone(&stop));
        let spec = spec.clone();
        tokio::task::spawn_blocking(move || selftest::run(&*b, &spec, emit, duration, &stop))
    };
    let joined = tokio::select! {
        r = &mut task => r,
        () = quit_signal() => {
            stop.store(true, Ordering::Release);
            task.await
        }
    };
    let report = joined
        .map_err(|e| CliError::Io(std::io::Error::other(e)))?
        .map_err(|e| match e {
            // A level above the self-test ceiling is a safety refusal; the rest is usage.
            SetupError::Level(m) => CliError::Refused(m),
            other => CliError::Usage(other.to_string()),
        })?;
    out.emit(&report, || text(&report))?;
    if report.pass {
        Ok(())
    } else {
        Err(CliError::CheckFailed)
    }
}

fn spread_ms(s: &Option<Spread>) -> String {
    match s {
        Some(s) => format!(
            "mean {:.3} ms, sd {:.3} ms, min {:.3} ms, max {:.3} ms",
            s.mean_us / 1e3,
            s.stddev_us / 1e3,
            s.min_us / 1e3,
            s.max_us / 1e3
        ),
        None => "—".into(),
    }
}

fn clock_text(c: ClockRelation) -> &'static str {
    match c {
        ClockRelation::SingleCallback => "one callback for both directions",
        ClockRelation::SameDeviceSeparateCallbacks => "one device, separate callbacks",
        ClockRelation::Unknown => "separate devices or endpoints (one clock not proven)",
    }
}

fn channels(list: &[u16]) -> String {
    crate::units::channels_text(list)
}

/// The report as text a tester can paste.
pub(crate) fn text(r: &DuplexReport) -> String {
    let mut s = String::new();
    let verdict = if r.pass { "PASS" } else { "FAIL" };
    let _ = writeln!(s, "ac2 selftest duplex: {verdict}");
    let _ = writeln!(
        s,
        "build        {BUILD_ID} ({} {})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(
        s,
        "run          {:.1} s, {}",
        r.seconds,
        match &r.emit {
            Some(e) => format!(
                "pink noise {:.1} dBFS, output {} → input {}",
                e.level_dbfs,
                e.loopback_out + 1,
                e.loopback_in + 1
            ),
            None => "outputs silent".into(),
        }
    );
    if let Some(o) = &r.opened {
        let _ = writeln!(
            s,
            "backend      {}",
            format!("{:?}", o.backend).to_lowercase()
        );
        let _ = writeln!(
            s,
            "input        {}: inputs {} of {}, {:?}",
            o.input_device,
            channels(&o.inputs),
            o.device_inputs,
            o.input_format
        );
        let _ = writeln!(
            s,
            "output       {}: {} outputs (device opened with {}){}",
            o.output_device,
            o.outputs,
            o.device_outputs,
            o.output_format
                .map(|f| format!(", {f:?}"))
                .unwrap_or_default()
        );
        let _ = writeln!(
            s,
            "stream       {} Hz, buffer {}",
            o.sample_rate,
            o.buffer_frames
                .map_or_else(|| "varies".to_owned(), |b| format!("{b} frames"))
        );
        let _ = writeln!(
            s,
            "clock        {}; sample index {}",
            clock_text(o.clock),
            match o.index {
                IndexExactness::Exact => "exact",
                IndexExactness::Estimated => "estimated from timestamps",
            }
        );
    }
    if let Some(c) = &r.capture {
        let sizes: Vec<String> = c
            .block_sizes
            .iter()
            .map(|(f, n)| format!("{f}×{n}"))
            .collect();
        let _ = writeln!(
            s,
            "blocks       {} ({} frames); sizes {}",
            c.blocks,
            c.frames,
            sizes.join(" ")
        );
        let _ = writeln!(
            s,
            "flags        xrun {}, discontinuity {}, overflow {}, config change {}, estimated \
             gap {}",
            c.xrun_blocks,
            c.discontinuity_blocks,
            c.overflow_blocks,
            c.config_change_blocks,
            c.estimated_gap_blocks
        );
        let _ = writeln!(
            s,
            "index        gaps {} ({} frames), regressions {}",
            c.index_gaps, c.gap_frames, c.index_regressions
        );
        let _ = writeln!(s, "in callback  {}", spread_ms(&c.callback_interval));
        let _ = writeln!(s, "in capture   {}", spread_ms(&c.capture_interval));
        let _ = writeln!(s, "input lag    {}", spread_ms(&c.input_lag));
        let peaks: Vec<String> = r
            .opened
            .as_ref()
            .map(|o| o.inputs.as_slice())
            .unwrap_or_default()
            .iter()
            .zip(&c.peak_dbfs)
            .map(|(ch, p)| match p {
                Some(db) => format!("{}: {db:.1}", ch + 1),
                None => format!("{}: -inf", ch + 1),
            })
            .collect();
        let _ = writeln!(s, "input peaks  {} dBFS", peaks.join(", "));
    }
    if let Some(o) = &r.output {
        let _ = writeln!(
            s,
            "out          {} callbacks ({} frames), discontinuities {}",
            o.callbacks, o.frames, o.discontinuities
        );
        let _ = writeln!(s, "out callback {}", spread_ms(&o.callback_interval));
        let _ = writeln!(s, "play lead    {}", spread_ms(&o.playback_lead));
    }
    let rate = |c: &Option<selftest::ClockRate>| match c {
        // Adding zero turns a rounded −0 into 0.
        Some(c) => format!(
            "{:.2} Hz ({:+} ppm over {:.1} s)",
            c.hz,
            c.ppm.round() + 0.0,
            c.span_s
        ),
        None => "—".into(),
    };
    if let (Some(c), Some(o)) = (&r.capture, &r.output) {
        let _ = writeln!(s, "input rate   {}", rate(&c.rate));
        let _ = writeln!(s, "output rate  {}", rate(&o.rate));
    }
    if let Some(e) = &r.host_events {
        let _ = writeln!(
            s,
            "host events  xruns {}, config changes {}, errors {}, ended {}",
            e.xruns,
            e.config_changes,
            e.errors,
            if e.ended { "yes" } else { "no" }
        );
    }
    if let Some(t) = &r.transport {
        let _ = writeln!(
            s,
            "transport    {} blocks, {} dropped",
            t.blocks_pushed, t.blocks_dropped
        );
    }
    if let Some(l) = &r.loopback {
        let _ = writeln!(
            s,
            "loopback     offset {}; locks {}, jumps {}, lost {}; windows {} ({} timed)",
            match (l.offset_samples, l.offset_ms) {
                (Some(o), Some(ms)) => format!("{o} samples ({ms:.3} ms)"),
                _ => "none".into(),
            },
            l.locks,
            l.jumps.len(),
            l.lost,
            l.windows,
            l.measured
        );
        let _ = writeln!(
            s,
            "drift        {}",
            match (l.drift_ppm, l.drift_span_s) {
                (Some(p), Some(span)) => format!("{p:+.2} ppm over {span:.0} s"),
                _ => "not judged (needs a longer run)".into(),
            }
        );
        if let Some(db) = l.loopback_dbfs {
            let _ = writeln!(
                s,
                "loop level   {db:.1} dBFS{}",
                l.psr_db
                    .map(|p| format!(", peak-to-sidelobe {p:.1} dB"))
                    .unwrap_or_default()
            );
        }
    }
    if let Some(stop) = r.stop {
        let _ = writeln!(s, "stop         {stop:?}");
    }
    if r.failures.is_empty() {
        let _ = writeln!(s, "result       PASS");
    } else {
        for f in &r.failures {
            let _ = writeln!(s, "FAIL         {f}");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_audio::DeviceId;

    fn device(id: &str, name: &str, inputs: u16, outputs: u16, default: bool) -> DeviceCaps {
        let dir = |n: u16| {
            (n > 0).then(|| ac2_audio::DirectionCaps {
                max_channels: n,
                rates: vec![],
                buffer_frames: None,
                formats: vec![],
                default_rate: None,
                default_buffer: None,
                channel_names: None,
                system_default: default,
            })
        };
        DeviceCaps {
            backend: ac2_audio::BackendKind::Fake,
            host: "test".into(),
            id: DeviceId(id.into()),
            name: name.into(),
            input: dir(inputs),
            output: dir(outputs),
            duplex_clock: ClockRelation::SingleCallback,
            index: IndexExactness::Exact,
            latency: ac2_audio::StaticLatency::Unknown,
            notes: vec![],
        }
    }

    fn args(extra: &[&str]) -> SelftestDuplex {
        use clap::Parser;
        let mut argv = vec!["ac2", "selftest", "duplex", "--backend", "fake"];
        argv.extend_from_slice(extra);
        match crate::Cli::parse_from(argv).cmd {
            crate::args::Cmd::Selftest {
                cmd: SelftestCmd::Duplex(a),
            } => a,
            other => panic!("parsed {other:?}"),
        }
    }

    #[test]
    fn defaults_take_every_input_of_the_default_device_and_two_outputs() {
        let devs = [
            device("a", "Mic", 1, 0, false),
            device("b", "Interface", 8, 8, true),
        ];
        let s = spec(&args(&[]), &devs).expect("spec");
        assert_eq!(s.input_map, (0..8).collect::<Vec<_>>());
        assert_eq!(s.output_channels, 2);
        assert_eq!(s.input_device, DeviceSelector::Default);

        let s = spec(&args(&["--device", "mic", "--in", "1"]), &devs).expect("by name");
        assert_eq!(s.input_device, DeviceSelector::Id(DeviceId("a".into())));
        assert_eq!(s.input_map, [0]);
        // The capture device plays nothing: the default output device does.
        assert_eq!(s.output_channels, 2);
        assert!(spec(&args(&["--device", "nope"]), &devs).is_err());
    }

    #[test]
    fn emit_needs_both_loopback_channels() {
        use clap::Parser;
        let base = ["ac2", "selftest", "duplex", "--backend", "fake"];
        let parse = |extra: &[&str]| {
            let mut v = base.to_vec();
            v.extend_from_slice(extra);
            crate::Cli::try_parse_from(v)
        };
        assert!(parse(&["--emit", "-40dbfs"]).is_err());
        assert!(parse(&["--loopback-out", "1"]).is_err());
        assert!(parse(&["--emit", "-40", "--loopback-out", "1", "--loopback-in", "1"]).is_err());
        assert!(
            parse(&[
                "--emit",
                "-40dbfs",
                "--loopback-out",
                "1",
                "--loopback-in",
                "1"
            ])
            .is_ok()
        );
    }
}
