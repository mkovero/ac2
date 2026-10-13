//! The figures of the health meter keepalives carry: whole-host CPU use, the session's
//! audio callback load and its xruns.
//!
//! Everything here runs on the control thread when a keepalive is due. The audio callback
//! only adds to counters ([`ac2_audio::StreamHealth`]); the windows, the session totals and
//! the CPU samples are made here, about once a second.

use std::time::{Duration, Instant};

use ac2_audio::{LoadTotals, StreamHealth};
use ac2_proto::frame::{AudioHealth, DaemonHealth};

/// How long one reading spans. A second holds hundreds of callbacks at any buffer size, so
/// the mean is steady, and an operator reads a number that changes once a second.
pub(crate) const WINDOW: Duration = Duration::from_secs(1);

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

/// The open session's audio across the streams it ran on: a recovery reopens the stream
/// within the session, and the totals keep counting across it.
struct SessionAudio {
    stream: StreamHealth,
    /// Totals of the session's earlier streams.
    carried: LoadTotals,
    carried_xruns: u64,
    /// The current stream's totals when the running window began.
    window_start: LoadTotals,
    session_peak: Option<f64>,
    shown: AudioHealth,
}

impl SessionAudio {
    /// Starts on a stream as it opened: what ran before the first look counts in the first
    /// window.
    fn new(stream: StreamHealth) -> Self {
        let xruns = stream.xruns();
        Self {
            stream,
            carried: LoadTotals::default(),
            carried_xruns: 0,
            window_start: LoadTotals::default(),
            session_peak: None,
            shown: AudioHealth {
                load: None,
                peak: None,
                session_load: None,
                session_peak: None,
                xruns,
            },
        }
    }

    /// The session's audio moved to `stream`.
    fn switch_to(&mut self, stream: StreamHealth) {
        let old = self.stream.load_totals();
        self.carried.busy_ns += old.busy_ns;
        self.carried.period_ns += old.period_ns;
        self.carried_xruns += self.stream.xruns();
        self.session_peak = max(self.session_peak, self.stream.take_peak());
        self.window_start = LoadTotals::default();
        self.stream = stream;
        self.shown.xruns = self.carried_xruns + self.stream.xruns();
    }

    /// Ends a window: the reading over it, and the session's totals.
    fn read(&mut self) {
        let now = self.stream.load_totals();
        let load = now.since(&self.window_start);
        let peak = self.stream.take_peak();
        self.window_start = now;
        self.session_peak = max(self.session_peak, peak);
        let total = LoadTotals {
            busy_ns: self.carried.busy_ns + now.busy_ns,
            period_ns: self.carried.period_ns + now.period_ns,
        };
        self.shown = AudioHealth {
            load,
            peak,
            session_load: total.since(&LoadTotals::default()),
            session_peak: self.session_peak,
            xruns: self.carried_xruns + self.stream.xruns(),
        };
    }
}

fn max(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// What the session's audio is, as the control thread knows it.
pub(crate) enum SessionStream<'a> {
    /// No session open.
    Closed,
    /// A session is open and runs on this stream.
    Running(&'a StreamHealth),
    /// A session is open but its stream stopped and is being reopened.
    Recovering,
}

/// The health meter's state.
pub(crate) struct Health {
    host: String,
    cpu: HostCpu,
    cpu_shown: Option<f64>,
    audio: Option<SessionAudio>,
    window_end: Option<Instant>,
}

impl Health {
    pub(crate) fn new() -> Self {
        Self {
            host: ac2_discovery::local_host_label(),
            cpu: HostCpu::new(),
            cpu_shown: None,
            audio: None,
            window_end: None,
        }
    }

    /// Brings the figures up to `now` for a session in state `session`: follows the stream
    /// at once, and ends a window when one is due.
    pub(crate) fn tick(&mut self, now: Instant, session: SessionStream<'_>) {
        match (session, &mut self.audio) {
            (SessionStream::Closed, a) => *a = None,
            (SessionStream::Running(s), Some(a)) if !a.stream.same_stream(s) => {
                a.switch_to(s.clone());
            }
            (SessionStream::Running(s), a @ None) => *a = Some(SessionAudio::new(s.clone())),
            (SessionStream::Running(_) | SessionStream::Recovering, _) => {}
        }
        if self.window_end.is_some_and(|t| now < t) {
            return;
        }
        self.window_end = Some(now + WINDOW);
        self.cpu_shown = self.cpu.sample();
        if let Some(a) = &mut self.audio {
            a.read();
        }
    }

    /// A new session (a client opened one): nothing of an earlier one carries over.
    pub(crate) fn session_closed(&mut self) {
        self.audio = None;
    }

    /// The figures to send.
    pub(crate) fn snapshot(&self) -> DaemonHealth {
        DaemonHealth {
            host: self.host.clone(),
            cpu: self.cpu_shown,
            audio: self.audio.as_ref().map(|a| a.shown),
        }
    }
}

#[cfg(test)]
mod tests {
    use ac2_audio::{DuplexRequest, DuplexStream, FakeBackend, FakeConfig, FakeDriver, MaxLevel};

    use super::*;

    fn fake_stream() -> (DuplexStream, FakeDriver) {
        let backend = FakeBackend::new(FakeConfig::default()).expect("fake");
        let req = DuplexRequest::new(vec![0], 1, MaxLevel::from_peak_db(-6.0).expect("level"));
        backend.open_manual(req).expect("open")
    }

    #[test]
    fn audio_figures_follow_the_session_and_carry_across_a_reopen() {
        let mut h = Health::new();
        let t0 = Instant::now();
        h.tick(t0, SessionStream::Closed);
        assert_eq!(h.snapshot().audio, None, "no session, no audio figures");

        let (a, mut drive_a) = fake_stream();
        h.tick(t0, SessionStream::Running(&a.health()));
        let first = h.snapshot().audio.expect("a session");
        assert_eq!(first.load, None, "no window ended yet: nothing, not zero");

        drive_a.run_blocks(20);
        h.tick(t0 + WINDOW, SessionStream::Running(&a.health()));
        let read = h.snapshot().audio.expect("a session");
        let load = read.load.expect("the fake device's callback ran");
        assert!((0.0..1.0).contains(&load), "{load}");
        assert!(read.session_load.is_some() && read.peak.is_some());

        // A recovery reopens the stream: the session's totals keep counting.
        h.tick(t0 + WINDOW, SessionStream::Recovering);
        let (b, _drive_b) = fake_stream();
        h.tick(t0 + 2 * WINDOW, SessionStream::Running(&b.health()));
        let after = h.snapshot().audio.expect("still the session");
        assert_eq!(after.load, None, "no block on the new stream yet");
        assert_eq!(
            after.session_load, read.session_load,
            "the first stream's blocks still count"
        );
        assert!(after.session_peak >= read.session_peak);

        h.session_closed();
        h.tick(t0 + 3 * WINDOW, SessionStream::Closed);
        assert_eq!(h.snapshot().audio, None);
    }

    #[test]
    fn the_host_is_named_and_cpu_needs_two_samples() {
        let mut h = Health::new();
        let t0 = Instant::now();
        h.tick(t0, SessionStream::Closed);
        let s = h.snapshot();
        assert!(!s.host.is_empty());
        assert_eq!(s.cpu, None, "one sample is a baseline, not a figure");
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        h.tick(t0 + WINDOW, SessionStream::Closed);
        let cpu = h.snapshot().cpu.expect("two samples");
        assert!((0.0..=1.0).contains(&cpu), "{cpu}");
    }
}
