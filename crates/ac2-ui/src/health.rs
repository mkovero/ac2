//! This window's side of the health meter: its frame rate, the bytes it receives from the
//! daemon, this computer's CPU use, and when the daemon's xrun count last rose. The scene
//! ([`ac2_scene::health`]) turns these and the daemon's own figures into the text.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ac2_proto::frame::DaemonHealth;
use ac2_scene::health::{HealthInputs, UiFrames};

/// How long one reading spans: as the daemon's, so both sides' figures move together.
pub const WINDOW: Duration = Duration::from_secs(1);

/// Whole-host CPU use, every process on every core.
struct HostCpu {
    sys: sysinfo::System,
    /// The first refresh only sets the baseline: use is a difference of two samples.
    primed: bool,
}

impl HostCpu {
    fn new() -> Self {
        Self {
            sys: sysinfo::System::new(),
            primed: false,
        }
    }

    /// Use since the previous call, as a share of capacity; `None` on the first call.
    fn sample(&mut self) -> Option<f64> {
        self.sys.refresh_cpu_usage();
        let primed = std::mem::replace(&mut self.primed, true);
        let pct = f64::from(self.sys.global_cpu_usage());
        (primed && pct.is_finite()).then(|| (pct / 100.0).clamp(0.0, 1.0))
    }
}

/// The window's own figures over the last [`WINDOW`].
#[derive(Debug, Default, Clone, Copy)]
struct Shown {
    cpu: Option<f64>,
    received_per_s: Option<f64>,
    frames: Option<(f64, Duration, bool)>,
}

/// This window's health figures.
pub struct LocalHealth {
    /// Bytes received from the daemon; the link's client adds to it.
    pub received: Arc<AtomicU64>,
    local_host: String,
    /// Sampled only while the daemon runs elsewhere: on this host its own figure says it.
    cpu: Option<HostCpu>,
    window_start: Instant,
    bytes_at_start: u64,
    frames: u32,
    longest_gap: Duration,
    last_frame: Option<Instant>,
    data: u32,
    /// Data arrived over the previous window too: the first window after an idle spell
    /// spans the idle time, and its rate would read as a stall.
    data_before: bool,
    shown: Shown,
    /// The daemon's xrun count as last seen, and when it last rose.
    xruns: Option<(u64, Option<Instant>)>,
}

impl std::fmt::Debug for LocalHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalHealth")
            .field("shown", &self.shown)
            .finish_non_exhaustive()
    }
}

impl Default for LocalHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalHealth {
    pub fn new() -> Self {
        Self {
            received: Arc::default(),
            local_host: ac2_discovery::local_host_label(),
            cpu: None,
            window_start: Instant::now(),
            bytes_at_start: 0,
            frames: 0,
            longest_gap: Duration::ZERO,
            last_frame: None,
            data: 0,
            data_before: false,
            shown: Shown::default(),
            xruns: None,
        }
    }

    /// This computer's host name.
    pub fn local_host(&self) -> &str {
        &self.local_host
    }

    /// Live data arrived from the link.
    pub fn data_arrived(&mut self) {
        self.data += 1;
    }

    /// A frame is drawn at `now`; `remote` says whether the daemon runs elsewhere, and so
    /// whether this computer's CPU is worth sampling.
    pub fn frame(&mut self, now: Instant, remote: bool) {
        if let Some(last) = self.last_frame {
            self.longest_gap = self.longest_gap.max(now.saturating_duration_since(last));
        }
        self.last_frame = Some(now);
        self.frames += 1;
        let span = now.saturating_duration_since(self.window_start);
        if span < WINDOW {
            return;
        }
        let secs = span.as_secs_f64();
        let bytes = self.received.load(Ordering::Relaxed);
        let streaming = self.data > 0 && self.data_before;
        self.shown = Shown {
            cpu: if remote {
                self.cpu.get_or_insert_with(HostCpu::new).sample()
            } else {
                self.cpu = None;
                None
            },
            received_per_s: Some(bytes.saturating_sub(self.bytes_at_start) as f64 / secs),
            frames: Some((f64::from(self.frames) / secs, self.longest_gap, streaming)),
        };
        self.data_before = self.data > 0;
        self.window_start = now;
        self.bytes_at_start = bytes;
        self.frames = 0;
        self.data = 0;
        self.longest_gap = Duration::ZERO;
    }

    /// Follows the daemon's xrun count: a rise starts the warning.
    pub fn daemon_xruns(&mut self, xruns: Option<u64>, now: Instant) {
        self.xruns = match (xruns, self.xruns) {
            (None, _) => None,
            (Some(n), Some((seen, _))) if n > seen => Some((n, Some(now))),
            // A new session or daemon counts from zero again: no rise.
            (Some(n), Some((_, at))) => Some((n, at)),
            (Some(n), None) => Some((n, None)),
        };
    }

    /// When the xrun warning ends, for a repaint then.
    pub fn xrun_warning_until(&self) -> Option<Instant> {
        let (_, at) = self.xruns?;
        at.map(|t| t + Duration::from_secs_f64(ac2_scene::health::XRUN_WARN_S))
    }

    /// What the scene builds the meter from.
    pub fn inputs<'a>(
        &'a self,
        daemon: Option<&'a DaemonHealth>,
        embedded: bool,
        wanted_fps: f64,
        now: Instant,
    ) -> HealthInputs<'a> {
        HealthInputs {
            daemon,
            embedded,
            local_host: &self.local_host,
            local_cpu: self.shown.cpu,
            received_per_s: self.shown.received_per_s,
            frames: self.shown.frames.map(|(fps, gap, streaming)| UiFrames {
                fps,
                longest_gap_s: gap.as_secs_f64(),
                streaming,
                wanted_fps,
            }),
            since_xrun_s: self
                .xruns
                .and_then(|(_, at)| at)
                .map(|t| now.saturating_duration_since(t).as_secs_f64()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rate_is_shown_as_streaming_only_after_a_whole_window_of_data() {
        let mut h = LocalHealth::new();
        let t0 = h.window_start;
        let frame_at =
            |h: &mut LocalHealth, ms: u64| h.frame(t0 + Duration::from_millis(ms), false);
        // An idle spell, then data: the window that spans the idle time is not a stall.
        h.data_arrived();
        frame_at(&mut h, 5000);
        let f = h
            .inputs(None, false, 30.0, t0)
            .frames
            .expect("a window ended");
        assert!(!f.streaming && f.fps < 1.0);
        for i in 1..=30 {
            h.data_arrived();
            frame_at(&mut h, 5000 + i * 34);
        }
        let f = h
            .inputs(None, false, 30.0, t0)
            .frames
            .expect("a window ended");
        assert!(f.streaming, "{f:?}");
        assert!((f.fps - 29.4).abs() < 0.5, "{f:?}");
        assert!((f.longest_gap_s - 0.034).abs() < 1e-9);
    }

    #[test]
    fn the_received_rate_is_bytes_over_the_window() {
        let mut h = LocalHealth::new();
        let t0 = h.window_start;
        h.received.fetch_add(2_000_000, Ordering::Relaxed);
        h.frame(t0 + Duration::from_secs(2), false);
        let r = h.inputs(None, false, 30.0, t0).received_per_s;
        assert_eq!(r, Some(1_000_000.0));
    }

    #[test]
    fn an_xrun_warns_from_the_rise_and_a_reset_count_is_no_rise() {
        let mut h = LocalHealth::new();
        let t0 = Instant::now();
        h.daemon_xruns(Some(4), t0);
        assert_eq!(
            h.inputs(None, false, 30.0, t0).since_xrun_s,
            None,
            "seen, not risen"
        );
        h.daemon_xruns(Some(5), t0 + Duration::from_secs(1));
        let since = h
            .inputs(None, false, 30.0, t0 + Duration::from_secs(3))
            .since_xrun_s;
        assert_eq!(since, Some(2.0));
        h.daemon_xruns(Some(0), t0 + Duration::from_secs(4));
        h.daemon_xruns(Some(0), t0 + Duration::from_secs(5));
        let at = h.xrun_warning_until().expect("the rise is kept");
        assert_eq!(at, t0 + Duration::from_secs(1) + Duration::from_secs(5));
    }
}
