//! The health meter after the session line: CPU use, the audio callback's load, xruns, the
//! link's data rate and the UI's frame rate (`CPU pupu 23 % · here 41 % · DSP 14 % ·
//! 0 xruns · 1.8 MB/s · 60 fps`).
//!
//! Every figure that is missing (an older daemon, no session, the first second) is left
//! out rather than shown as zero: a zero reads as a measurement.

use ac2_proto::frame::DaemonHealth;

use crate::format;

/// CPU use, whole host, from which a figure reads as a warning: past it a busy host starts
/// to delay the audio thread's wake-ups.
pub const CPU_WARN: f64 = 0.85;
/// Callback load from which a figure reads as a warning: past it a callback that runs a
/// little long (a cache miss, an interrupt) misses its period.
pub const DSP_WARN: f64 = 0.70;
/// How long the xrun count stays a warning after it rose, seconds.
pub const XRUN_WARN_S: f64 = 5.0;
/// The frame rate warns below this share of the rate the UI asked its data at: a UI drawing
/// a third of what arrives is stalling (a blanked screen, a starved compositor), not idle.
pub const FPS_WARN_SHARE: f64 = 1.0 / 3.0;

/// How the UI drew over the last second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiFrames {
    /// Frames drawn per second.
    pub fps: f64,
    /// The longest time between two frames, seconds.
    pub longest_gap_s: f64,
    /// Whether live data arrived over the same second: an idle UI draws only when
    /// something changes, so a low rate means something only while data streams.
    pub streaming: bool,
    /// Frames a second the UI asked its data at.
    pub wanted_fps: f64,
}

/// What the meter is built from.
#[derive(Clone, Copy, Debug)]
pub struct HealthInputs<'a> {
    /// The daemon's figures from its latest keepalive.
    pub daemon: Option<&'a DaemonHealth>,
    /// The daemon runs inside this process: no network between, and the same host.
    pub embedded: bool,
    /// This machine's host name.
    pub local_host: &'a str,
    /// This machine's CPU use, share of capacity, when sampled.
    pub local_cpu: Option<f64>,
    /// Bytes a second received from the daemon over the last second; shown only while the
    /// daemon's figures are.
    pub received_per_s: Option<f64>,
    /// How this window drew over the last second.
    pub frames: Option<UiFrames>,
    /// Seconds since the daemon's xrun count last rose, as this UI saw it.
    pub since_xrun_s: Option<f64>,
}

/// One figure of the meter.
#[derive(Clone, Debug, PartialEq)]
pub struct HealthPart {
    pub text: String,
    /// Kept when the bar is short of room.
    pub essential: bool,
    /// Shown in the warning colour.
    pub warn: bool,
}

/// The meter: its figures in order, and the detail behind them.
#[derive(Clone, Debug, PartialEq)]
pub struct HealthLine {
    pub parts: Vec<HealthPart>,
    /// One line per source; names the host the daemon's figures come from.
    pub detail: String,
}

impl HealthLine {
    /// Every figure, as the bar shows them with room to spare.
    pub fn text(&self) -> String {
        self.join(|_| true)
    }

    /// The essential figures only.
    pub fn short(&self) -> String {
        self.join(|p| p.essential)
    }

    fn join(&self, keep: impl Fn(&HealthPart) -> bool) -> String {
        self.parts
            .iter()
            .filter(|p| keep(p))
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// `0.14` → `14 %`.
pub fn percent(share: f64) -> String {
    format!("{} %", format::fixed(share * 100.0, 0))
}

/// `1.8 MB/s`, `340 kB/s`, `800 B/s` (decimal prefixes, as network rates are stated).
pub fn byte_rate(bytes_per_s: f64) -> String {
    let b = bytes_per_s.max(0.0);
    let (v, unit) = if b >= 1e6 {
        (b / 1e6, "MB/s")
    } else if b >= 1e3 {
        (b / 1e3, "kB/s")
    } else {
        return format!("{} B/s", format::fixed(b, 0));
    };
    let decimals = if v < 10.0 { 1 } else { 0 };
    format!("{} {unit}", format::fixed(v, decimals))
}

fn xruns_text(n: u64) -> String {
    if n == 1 {
        "1 xrun".into()
    } else {
        format!("{n} xruns")
    }
}

fn part(text: String, essential: bool, warn: bool) -> HealthPart {
    HealthPart {
        text,
        essential,
        warn,
    }
}

/// The meter for `i`; `None` when there is no figure to show.
pub fn health_line(i: &HealthInputs) -> Option<HealthLine> {
    let mut parts = Vec::new();
    let mut detail = Vec::new();
    let daemon = i.daemon;
    // Two CPU figures only when the daemon runs on another machine: the same host's twice
    // would say the same thing.
    let remote = daemon.is_some_and(|d| !i.embedded && d.host != i.local_host);
    let local_cpu = i.local_cpu.filter(|_| remote);

    if let Some(d) = daemon {
        let mut lines = Vec::new();
        if let Some(c) = d.cpu {
            let text = if remote {
                format!("CPU {} {}", d.host, percent(c))
            } else {
                format!("CPU {}", percent(c))
            };
            parts.push(part(text, true, c >= CPU_WARN));
            lines.push(format!("CPU {} (every process on every core)", percent(c)));
        }
        if let Some(cpu) = local_cpu {
            let text = if d.cpu.is_some() {
                format!("here {}", percent(cpu))
            } else {
                format!("CPU here {}", percent(cpu))
            };
            parts.push(part(text, true, cpu >= CPU_WARN));
        }
        if let Some(a) = d.audio {
            if let Some(l) = a.load {
                parts.push(part(format!("DSP {}", percent(l)), true, l >= DSP_WARN));
            }
            let warn = i.since_xrun_s.is_some_and(|s| s < XRUN_WARN_S);
            parts.push(part(xruns_text(a.xruns), true, warn));
            let span = |mean: Option<f64>, peak: Option<f64>| match (mean, peak) {
                (Some(m), Some(p)) => Some(format!("{} mean, {} peak", percent(m), percent(p))),
                (Some(m), None) => Some(format!("{} mean", percent(m))),
                (None, Some(p)) => Some(format!("{} peak", percent(p))),
                (None, None) => None,
            };
            if let Some(s) = span(a.load, a.peak) {
                lines.push(format!("DSP {s} over the last second"));
            }
            if let Some(s) = span(a.session_load, a.session_peak) {
                lines.push(format!("DSP {s} since the session opened"));
            }
            lines.push(format!("{} since the session opened", xruns_text(a.xruns)));
            lines.push(
                "DSP: time in ac2d's own audio callback against the buffer period, not the \
                 whole audio graph's load"
                    .into(),
            );
        }
        if !lines.is_empty() {
            let from = if remote {
                format!("{} (the daemon's host)", d.host)
            } else if i.embedded {
                format!("{} (this computer, daemon in this app)", d.host)
            } else {
                format!("{} (this computer)", d.host)
            };
            detail.push(format!("{from}:\n  {}", lines.join("\n  ")));
        }
    }

    let mut here = Vec::new();
    if let Some(cpu) = local_cpu {
        here.push(format!(
            "CPU {} (every process on every core)",
            percent(cpu)
        ));
    }
    // Only while the daemon answers: a rate without a link is a zero that says nothing.
    if let Some(r) = i.received_per_s.filter(|_| !i.embedded && daemon.is_some()) {
        parts.push(part(byte_rate(r), false, false));
        here.push(format!("{} received from the daemon", byte_rate(r)));
    }
    if let Some(f) = i.frames.filter(|f| f.streaming) {
        let warn = f.fps < f.wanted_fps * FPS_WARN_SHARE;
        parts.push(part(
            format!("{} fps", format::fixed(f.fps, 0)),
            false,
            warn,
        ));
        here.push(format!(
            "{} frames a second drawn, {} asked for; longest gap between frames {}",
            format::fixed(f.fps, 0),
            format::fixed(f.wanted_fps, 0),
            format::ms(f.longest_gap_s, 0)
        ));
    }
    if !here.is_empty() {
        let name = if remote {
            format!("{} (this computer)", i.local_host)
        } else {
            "this window".to_owned()
        };
        detail.push(format!("{name}:\n  {}", here.join("\n  ")));
    }
    (!parts.is_empty()).then(|| HealthLine {
        parts,
        detail: detail.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use ac2_proto::frame::AudioHealth;

    use super::*;

    fn daemon(host: &str, cpu: Option<f64>, audio: Option<AudioHealth>) -> DaemonHealth {
        DaemonHealth {
            host: host.into(),
            cpu,
            audio,
        }
    }

    fn audio(load: Option<f64>, xruns: u64) -> AudioHealth {
        AudioHealth {
            load,
            peak: load.map(|l| l * 2.0),
            session_load: Some(0.12),
            session_peak: Some(0.48),
            xruns,
        }
    }

    fn inputs<'a>(d: Option<&'a DaemonHealth>, local_host: &'a str) -> HealthInputs<'a> {
        HealthInputs {
            daemon: d,
            embedded: false,
            local_host,
            local_cpu: None,
            received_per_s: None,
            frames: None,
            since_xrun_s: None,
        }
    }

    fn streaming(fps: f64) -> UiFrames {
        UiFrames {
            fps,
            longest_gap_s: 0.034,
            streaming: true,
            wanted_fps: 30.0,
        }
    }

    #[test]
    fn a_remote_daemon_names_its_host_beside_this_computers_cpu() {
        let d = daemon("pupu", Some(0.23), Some(audio(Some(0.14), 0)));
        let i = HealthInputs {
            local_cpu: Some(0.41),
            received_per_s: Some(1.8e6),
            frames: Some(streaming(60.0)),
            ..inputs(Some(&d), "ketunkolo")
        };
        let l = health_line(&i).expect("figures");
        assert_eq!(
            l.text(),
            "CPU pupu 23 % · here 41 % · DSP 14 % · 0 xruns · 1.8 MB/s · 60 fps"
        );
        assert_eq!(l.short(), "CPU pupu 23 % · here 41 % · DSP 14 % · 0 xruns");
        assert!(l.parts.iter().all(|p| !p.warn));
        assert!(
            l.detail.starts_with("pupu (the daemon's host):\n"),
            "{}",
            l.detail
        );
        for want in [
            "DSP 14 % mean, 28 % peak over the last second",
            "DSP 12 % mean, 48 % peak since the session opened",
            "0 xruns since the session opened",
            "not the whole audio graph",
            "ketunkolo (this computer):\n  CPU 41 %",
            "1.8 MB/s received from the daemon",
            "longest gap between frames 34 ms",
        ] {
            assert!(l.detail.contains(want), "{want:?} in {}", l.detail);
        }
    }

    #[test]
    fn an_embedded_daemon_shows_one_cpu_and_no_network() {
        let d = daemon("ketunkolo", Some(0.23), Some(audio(Some(0.14), 2)));
        let i = HealthInputs {
            embedded: true,
            local_cpu: Some(0.41),
            received_per_s: Some(5e3),
            ..inputs(Some(&d), "ketunkolo")
        };
        let l = health_line(&i).expect("figures");
        assert_eq!(l.text(), "CPU 23 % · DSP 14 % · 2 xruns");
        assert!(l.detail.contains("daemon in this app"), "{}", l.detail);
        // A local daemon on the same host is not counted twice either.
        let i = HealthInputs {
            local_cpu: Some(0.41),
            ..inputs(Some(&d), "ketunkolo")
        };
        assert_eq!(
            health_line(&i).expect("figures").text(),
            "CPU 23 % · DSP 14 % · 2 xruns"
        );
    }

    #[test]
    fn missing_figures_are_left_out_not_zero() {
        assert_eq!(health_line(&inputs(None, "here")), None, "nothing at all");
        let d = daemon("pupu", None, None);
        assert_eq!(
            health_line(&inputs(Some(&d), "here")),
            None,
            "no cpu, no session"
        );
        let d = daemon("pupu", None, Some(audio(None, 0)));
        let l = health_line(&inputs(Some(&d), "here")).expect("xruns");
        assert_eq!(l.text(), "0 xruns", "no load before the first second");
        let i = HealthInputs {
            frames: Some(UiFrames {
                streaming: false,
                ..streaming(2.0)
            }),
            ..inputs(Some(&d), "here")
        };
        assert_eq!(
            health_line(&i).expect("xruns").text(),
            "0 xruns",
            "an idle UI draws on demand: its rate says nothing"
        );
    }

    #[test]
    fn figures_warn_past_their_limits_and_xruns_only_for_a_while() {
        let d = daemon("pupu", Some(0.9), Some(audio(Some(0.75), 1)));
        let i = HealthInputs {
            local_cpu: Some(0.5),
            frames: Some(streaming(1.0)),
            since_xrun_s: Some(1.0),
            ..inputs(Some(&d), "ketunkolo")
        };
        let l = health_line(&i).expect("figures");
        let warned: Vec<&str> = l
            .parts
            .iter()
            .filter(|p| p.warn)
            .map(|p| p.text.as_str())
            .collect();
        assert_eq!(warned, ["CPU pupu 90 %", "DSP 75 %", "1 xrun", "1 fps"]);
        let later = HealthInputs {
            since_xrun_s: Some(XRUN_WARN_S + 0.1),
            frames: Some(streaming(25.0)),
            ..i
        };
        let l = health_line(&later).expect("figures");
        assert!(l.parts.iter().filter(|p| p.warn).count() == 2, "{l:?}");
    }

    #[test]
    fn byte_rates_read_with_decimal_prefixes() {
        assert_eq!(byte_rate(800.0), "800 B/s");
        assert_eq!(byte_rate(1_234.0), "1.2 kB/s");
        assert_eq!(byte_rate(340_000.0), "340 kB/s");
        assert_eq!(byte_rate(1_840_000.0), "1.8 MB/s");
        assert_eq!(byte_rate(12_600_000.0), "13 MB/s");
    }
}
