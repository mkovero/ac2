//! `selftest stamps`: the capture-only timestamp log ([`crate::selftest::stamps`]) and its
//! closing summary.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::selftest::{backend, capture, enumerate};
use crate::args::SelftestStamps;
use crate::output::Out;
use crate::selftest::stamps::{self, Provenance, StampsError, StampsMeta, StampsSpec};
use crate::watch::quit_signal;
use crate::{BUILD_ID, CliError};

/// Longest run: a day of records is 2 GB at 96 kHz / 128 frames.
const MAX_MINUTES: f64 = 24.0 * 60.0;

/// JACK client name, apart from the daemon's `ac2` in the graph.
const CLIENT: &str = "ac2-stamps";

pub(super) async fn stamps(a: &SelftestStamps, out: &mut Out<'_>) -> Result<(), CliError> {
    if !(a.minutes.is_finite() && a.minutes > 0.0 && a.minutes <= MAX_MINUTES) {
        return Err(CliError::Usage(format!(
            "--minutes: more than 0 and at most {MAX_MINUTES:.0}"
        )));
    }
    let backend = backend(a.capture.backend, CLIENT)?;
    let devices = enumerate(&backend).await?;
    let c = capture(&a.capture, &devices)?;
    let spec = StampsSpec {
        input_device: c.selector,
        input_map: c.input_map,
        sample_rate: c.rate,
        buffer_frames: c.buffer,
        keep: a.keep.map(|k| k.0),
    };
    let provenance = Provenance {
        host: ac2_discovery::local_host_label(),
        ac2_build: BUILD_ID.into(),
    };
    let duration = Duration::from_secs_f64(a.minutes * 60.0);
    eprintln!(
        "ac2: logging stamps for {} min into {}, no outputs opened (Ctrl-C ends early)",
        a.minutes,
        a.out.display()
    );
    let stop = Arc::new(AtomicBool::new(false));
    let mut task = {
        let (b, stop, dir) = (Arc::clone(&backend), Arc::clone(&stop), a.out.clone());
        tokio::task::spawn_blocking(move || {
            stamps::run(&*b, &spec, &dir, provenance, duration, &stop)
        })
    };
    let joined = tokio::select! {
        r = &mut task => r,
        () = quit_signal() => {
            stop.store(true, Ordering::Release);
            task.await
        }
    };
    let meta = joined
        .map_err(|e| CliError::Io(std::io::Error::other(e)))?
        .map_err(|e| match e {
            StampsError::Io(e) => CliError::Io(e),
            other => CliError::Usage(other.to_string()),
        })?;
    out.emit(&meta, || text(&meta))?;
    Ok(())
}

/// The closing summary a tester can paste.
pub(crate) fn text(m: &StampsMeta) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "ac2 selftest stamps: {} {} at {} Hz, {} frames",
        m.provenance.host,
        m.device,
        m.sample_rate,
        m.buffer_frames
            .map_or_else(|| "variable".into(), |b| b.to_string())
    );
    if let Some([min, max]) = m.capture_latency_frames {
        let _ = writeln!(s, "  capture latency  {min}–{max} frames");
    }
    let _ = writeln!(
        s,
        "  stamp clock      {} ({})",
        m.stamp_clock.as_deref().unwrap_or("none"),
        m.stamp_clock_reads_as.unwrap_or("not identified")
    );
    let Some(e) = &m.end else {
        return s;
    };
    let c = &e.capture;
    let _ = writeln!(
        s,
        "  ran              {:.1} s{}",
        e.seconds,
        if e.interrupted { " (interrupted)" } else { "" }
    );
    let _ = writeln!(s, "  blocks           {} ({} frames)", c.blocks, c.frames);
    let _ = writeln!(
        s,
        "  gaps / xruns     {} gaps ({} frames), {} xrun blocks, {} host xruns, {} dropped blocks",
        c.index_gaps, c.gap_frames, c.xrun_blocks, e.host_events.xruns, e.transport.blocks_dropped
    );
    let _ = writeln!(
        s,
        "  pairs            {} kept, {} dropped (bracket > {} µs), {} unavailable",
        e.pairs.kept,
        e.pairs.dropped,
        stamps::MAX_BRACKET_NS / 1000,
        e.pairs.unavailable
    );
    match (e.fit.rate, e.fit.residual_sigma_ns) {
        (Some(r), Some(sigma)) => {
            let _ = writeln!(
                s,
                "  capture_ns fit   {:+.3} ppm against the stamp clock, residual σ {:.0} ns \
                 ({} blocks, {} contiguous stretch(es))",
                r.ppm, sigma, e.fit.points, e.fit.segments
            );
        }
        _ => {
            let _ = writeln!(s, "  capture_ns fit   — (too few capture stamps)");
        }
    }
    if let Some(k) = &m.keep {
        let _ = writeln!(
            s,
            "  kept input {}     {} frames in {}",
            k.channel, k.frames, k.file
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::Cli;
    use crate::output::Out;

    fn cli(dir: &std::path::Path, minutes: &str, extra: &[&str]) -> Cli {
        let dir = dir.to_str().expect("utf-8 path");
        let mut argv = vec![
            "ac2",
            "selftest",
            "stamps",
            "--backend",
            "fake",
            "--in",
            "1-2",
            "--minutes",
            minutes,
            "--out",
            dir,
        ];
        argv.extend_from_slice(extra);
        Cli::parse_from(argv)
    }

    /// The whole command on the fake: a second's log with a kept input, then the summary.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_command_logs_and_summarises() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("run");
        let mut so = Vec::new();
        let mut out = Out::new(false, &mut so);
        crate::run(&cli(&dir, "0.02", &["--keep", "2"]), &mut out)
            .await
            .expect("run");
        let text = String::from_utf8(so).expect("utf-8");
        assert!(text.starts_with("ac2 selftest stamps:"), "{text}");
        assert!(text.contains("capture_ns fit"), "{text}");
        assert!(text.contains("kept input 2"), "{text}");
        for f in ["blocks.bin", "pairs.csv", "meta.json", "keep.wav"] {
            assert!(dir.join(f).is_file(), "{f}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_run_length_out_of_range_is_refused() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut so = Vec::new();
        let mut out = Out::new(false, &mut so);
        assert!(
            crate::run(&cli(tmp.path(), "0", &[]), &mut out)
                .await
                .is_err()
        );
        assert!(!tmp.path().join("blocks.bin").exists());
    }
}
