//! `selftest frames`: how one measurement's live frames reach this client. Subscribes to
//! its stream exactly as the app does and logs every frame the data connection reads
//! ([`ac2_client::ClientConfig::record_arrivals`]), never only the newest of a drain.
//!
//! Arrival times are read as the drain takes each message, and the drain runs as soon as
//! the data socket signals, so they trail the kernel's receipt by the wake-up latency only.
//! The age is this host's wall clock at arrival minus the frame's `capture_wall_ns` (the
//! daemon host's wall clock at the frame's last sample): over the network it holds the two
//! clocks' offset as well as the delivery delay, so its drift over a run is the clocks'
//! relative drift plus any queue that builds.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use ac2_client::{Arrival, Latest};
use ac2_proto::{Subscription, Topic};
use serde::Serialize;

use crate::CliError;
use crate::args::{Cli, SelftestFrames};
use crate::cmd::{connect_with, find_meas, rate};
use crate::output::Out;
use crate::watch::{dead, quit_signal};

/// Period of the running text summary.
const REPORT_EVERY: Duration = Duration::from_secs(5);

/// Longest run: a day.
const MAX_SECONDS: f64 = 24.0 * 3600.0;

/// How long the state may take to arrive after the connect.
const SYNC_TIMEOUT: Duration = Duration::from_secs(5);

/// One frame as read, with what it says relative to the frame before it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct FrameRow {
    /// The frame's `seq`.
    pub seq: u64,
    /// A keyframe, not a delta.
    pub keyframe: bool,
    /// Applied on the frame held (a refused delta or a stale frame is not).
    pub applied: bool,
    /// A `seq` not newer than the one before (the same frame sent again).
    pub repeat: bool,
    /// Message bytes.
    pub bytes: usize,
    /// Seconds since the run began, monotonic clock.
    pub t_s: f64,
    /// Arrival on this host's wall clock, ns since the Unix epoch.
    pub arrival_wall_ns: i128,
    /// The frame's `capture_wall_ns` (daemon host's wall clock).
    pub capture_wall_ns: Option<u64>,
    /// The frame's `audio_sample`.
    pub audio_sample: Option<u64>,
    /// Time since the previous new frame arrived, ms.
    pub interval_ms: Option<f64>,
    /// `arrival_wall_ns − capture_wall_ns`, ms (includes the clock offset).
    pub age_ms: Option<f64>,
    /// `audio_sample` step from the previous new frame, ms at the session rate: the hop.
    pub hop_ms: Option<f64>,
    /// Frames between this one and the previous new one that never arrived.
    pub seq_missing: u64,
}

/// Turns arrivals into rows; remembers the last new frame.
#[derive(Debug)]
pub(crate) struct Tracker {
    start: Instant,
    rate: Option<u32>,
    last: Option<(f64, u64, Option<u64>)>,
}

impl Tracker {
    pub(crate) fn new(start: Instant, rate: Option<u32>) -> Self {
        Self {
            start,
            rate,
            last: None,
        }
    }

    pub(crate) fn row(&mut self, a: &Arrival) -> FrameRow {
        let t_s = a.at.saturating_duration_since(self.start).as_secs_f64();
        let capture = a.stamp.map(|s| s.capture_wall_ns.0);
        let sample = a.stamp.map(|s| s.audio_sample.0);
        let mut row = FrameRow {
            seq: a.seq,
            keyframe: a.keyframe,
            applied: a.stamp.is_some(),
            repeat: false,
            bytes: a.bytes,
            t_s,
            arrival_wall_ns: a.wall_ns,
            capture_wall_ns: capture,
            audio_sample: sample,
            interval_ms: None,
            age_ms: capture.map(|c| (a.wall_ns - i128::from(c)) as f64 / 1e6),
            hop_ms: None,
            seq_missing: 0,
        };
        if !row.applied {
            return row;
        }
        if let Some((t0, seq0, sample0)) = self.last {
            if a.seq <= seq0 {
                row.repeat = true;
                return row;
            }
            row.interval_ms = Some((t_s - t0) * 1e3);
            row.seq_missing = a.seq - seq0 - 1;
            row.hop_ms = match (sample0, sample, self.rate) {
                (Some(s0), Some(s1), Some(r)) if s1 >= s0 && r > 0 => {
                    Some((s1 - s0) as f64 / f64::from(r) * 1e3)
                }
                _ => None,
            };
        }
        self.last = Some((t_s, a.seq, sample));
        row
    }
}

/// Rows of a stretch of the run.
#[derive(Debug, Default)]
pub(crate) struct Record {
    from_s: f64,
    new: u64,
    repeats: u64,
    unapplied: u64,
    keyframes: u64,
    deltas: u64,
    intervals: Vec<f64>,
    /// `(t_s, age_ms)`.
    ages: Vec<(f64, f64)>,
    hops: Vec<f64>,
    missing: u64,
    gap_events: u64,
    bytes: Vec<f64>,
}

/// `mean`, percentiles and `max` of a set of values.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct Spread {
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub min: f64,
    pub max: f64,
}

/// Nearest-rank percentile of sorted values.
fn pct(sorted: &[f64], p: f64) -> f64 {
    let i = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[i.clamp(1, sorted.len()) - 1]
}

fn spread(v: &[f64]) -> Option<Spread> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    Some(Spread {
        mean: s.iter().sum::<f64>() / s.len() as f64,
        p50: pct(&s, 50.0),
        p95: pct(&s, 95.0),
        p99: pct(&s, 99.0),
        min: s[0],
        max: s[s.len() - 1],
    })
}

/// Least-squares slope of `y` over `x`, per unit of `x`.
fn slope(pts: &[(f64, f64)]) -> Option<f64> {
    if pts.len() < 2 {
        return None;
    }
    let n = pts.len() as f64;
    let (mx, my) = pts
        .iter()
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x / n, b + y / n));
    let (sxy, sxx) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| {
        (a + (x - mx) * (y - my), b + (x - mx) * (x - mx))
    });
    (sxx > 0.0).then(|| sxy / sxx)
}

/// What a stretch of the run came to.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct Summary {
    /// Length of the stretch, s.
    pub seconds: f64,
    /// Frames with a new `seq`.
    pub frames: u64,
    /// The same frame again (not newer than the one before).
    pub repeats: u64,
    /// Not applied: a delta refused, a frame of an older epoch or incarnation.
    pub unapplied: u64,
    pub keyframes: u64,
    pub deltas: u64,
    /// Arrival spacing of new frames, ms.
    pub interval_ms: Option<Spread>,
    /// Intervals longer than twice the median.
    pub gaps_over_2x_median: u64,
    /// Age at arrival, ms (includes the clock offset between the two hosts).
    pub age_ms: Option<Spread>,
    /// Least-squares trend of the age over the stretch, ms per minute.
    pub age_drift_ms_per_min: Option<f64>,
    /// Frames the client never saw (`seq` gaps), and in how many gaps.
    pub seq_missing: u64,
    pub seq_gap_events: u64,
    /// `audio_sample` step between new frames at the session rate, ms: the daemon's hop.
    pub hop_ms: Option<Spread>,
    /// Bytes per message.
    pub bytes: Option<Spread>,
    /// Every message's bytes over the stretch, Mbit/s.
    pub mbit_s: f64,
}

impl Record {
    fn starting(from_s: f64) -> Self {
        Self {
            from_s,
            ..Self::default()
        }
    }

    pub(crate) fn push(&mut self, r: &FrameRow) {
        self.bytes.push(r.bytes as f64);
        if r.keyframe {
            self.keyframes += 1;
        } else {
            self.deltas += 1;
        }
        if !r.applied {
            self.unapplied += 1;
            return;
        }
        if r.repeat {
            self.repeats += 1;
            return;
        }
        self.new += 1;
        self.intervals.extend(r.interval_ms);
        self.hops.extend(r.hop_ms);
        if let Some(a) = r.age_ms {
            self.ages.push((r.t_s, a));
        }
        self.missing += r.seq_missing;
        self.gap_events += u64::from(r.seq_missing > 0);
    }

    pub(crate) fn summary(&self, to_s: f64) -> Summary {
        let seconds = (to_s - self.from_s).max(0.0);
        let interval = spread(&self.intervals);
        let gaps = interval.as_ref().map_or(0, |s| {
            self.intervals.iter().filter(|&&i| i > 2.0 * s.p50).count() as u64
        });
        let ages: Vec<f64> = self.ages.iter().map(|a| a.1).collect();
        let total: f64 = self.bytes.iter().sum();
        Summary {
            seconds,
            frames: self.new,
            repeats: self.repeats,
            unapplied: self.unapplied,
            keyframes: self.keyframes,
            deltas: self.deltas,
            interval_ms: interval,
            gaps_over_2x_median: gaps,
            age_ms: spread(&ages),
            age_drift_ms_per_min: slope(&self.ages).map(|s| s * 60.0),
            seq_missing: self.missing,
            seq_gap_events: self.gap_events,
            hop_ms: spread(&self.hops),
            bytes: spread(&self.bytes),
            mbit_s: if seconds > 0.0 {
                total * 8.0 / seconds / 1e6
            } else {
                0.0
            },
        }
    }
}

/// The closing JSON object.
#[derive(Debug, Serialize)]
struct Closing<'a> {
    summary: &'a Summary,
    meas: &'a str,
    topic: String,
    rate_hz: Option<u32>,
    /// The client's keepalive estimate of `daemon − local` wall clock, ms.
    clock_offset_ms: Option<f64>,
    note: &'static str,
}

const AGE_NOTE: &str = "age = arrival wall clock here − capture_wall_ns on the daemon host; \
                        it includes the offset between the two clocks (NTP: about ±1 ms)";

fn ms(s: &Option<Spread>, f: impl Fn(&Spread) -> f64) -> String {
    s.as_ref()
        .map_or_else(|| "—".into(), |s| format!("{:.1}", f(s)))
}

/// One line for the running summary.
pub(crate) fn line(s: &Summary, to_s: f64) -> String {
    format!(
        "{to_s:6.1} s  {} frames  interval p50 {} p99 {} max {} ms  age p50 {} max {} ms  \
         hop {} ms  missing {}  {:.2} Mbit/s",
        s.frames,
        ms(&s.interval_ms, |x| x.p50),
        ms(&s.interval_ms, |x| x.p99),
        ms(&s.interval_ms, |x| x.max),
        ms(&s.age_ms, |x| x.p50),
        ms(&s.age_ms, |x| x.max),
        ms(&s.hop_ms, |x| x.p50),
        s.seq_missing,
        s.mbit_s,
    )
}

/// The closing summary.
fn text(c: &Closing<'_>) -> String {
    let s = c.summary;
    let mut o = String::new();
    let _ = writeln!(
        o,
        "ac2 selftest frames: {} ({}), {:.1} s",
        c.meas, c.topic, s.seconds
    );
    let _ = writeln!(
        o,
        "  frames           {} new, {} repeats, {} not applied ({} keyframes, {} deltas)",
        s.frames, s.repeats, s.unapplied, s.keyframes, s.deltas
    );
    let i = &s.interval_ms;
    let _ = writeln!(
        o,
        "  interval         mean {} p50 {} p95 {} p99 {} max {} ms, {} gaps > 2× median",
        ms(i, |x| x.mean),
        ms(i, |x| x.p50),
        ms(i, |x| x.p95),
        ms(i, |x| x.p99),
        ms(i, |x| x.max),
        s.gaps_over_2x_median
    );
    let h = &s.hop_ms;
    let _ = writeln!(
        o,
        "  hop (audio)      mean {} p50 {} min {} max {} ms at {}",
        ms(h, |x| x.mean),
        ms(h, |x| x.p50),
        ms(h, |x| x.min),
        ms(h, |x| x.max),
        c.rate_hz
            .map_or_else(|| "no open session".into(), |r| format!("{r} Hz"))
    );
    let a = &s.age_ms;
    let _ = writeln!(
        o,
        "  age              min {} p50 {} p95 {} max {} ms, drift {} ms/min",
        ms(a, |x| x.min),
        ms(a, |x| x.p50),
        ms(a, |x| x.p95),
        ms(a, |x| x.max),
        s.age_drift_ms_per_min
            .map_or_else(|| "—".into(), |d| format!("{d:+.2}"))
    );
    let _ = writeln!(
        o,
        "  seq gaps         {} frames missing in {} gaps",
        s.seq_missing, s.seq_gap_events
    );
    let b = &s.bytes;
    let _ = writeln!(
        o,
        "  size             mean {} p50 {} max {} bytes, {:.3} Mbit/s",
        ms(b, |x| x.mean),
        ms(b, |x| x.p50),
        ms(b, |x| x.max),
        s.mbit_s
    );
    if let Some(off) = c.clock_offset_ms {
        let _ = writeln!(
            o,
            "  clock offset     daemon − local ≈ {off:+.1} ms (keepalive estimate)"
        );
    }
    let _ = writeln!(o, "  note             {}", c.note);
    o
}

pub(super) async fn frames(
    cli: &Cli,
    a: &SelftestFrames,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    if !(a.seconds.is_finite() && a.seconds > 0.0 && a.seconds <= MAX_SECONDS) {
        return Err(CliError::Usage(format!(
            "--seconds: more than 0 and at most {MAX_SECONDS:.0}"
        )));
    }
    let c = connect_with(cli, |cfg| {
        cfg.mirror = true;
        cfg.record_arrivals = true;
    })
    .await?;
    let state = c.wait_synced(SYNC_TIMEOUT).await?;
    let m = find_meas(&state, &a.meas)?;
    let Some(stream) = m.config.kind.stream() else {
        return Err(CliError::Usage(format!(
            "{:?} publishes no live stream (a sweep's results are stored traces)",
            m.config.name
        )));
    };
    let topic = Topic::Data { meas: m.id, stream };
    let name = m.config.name.clone();
    let rate_hz = rate(&state);
    let sub = Subscription::Topic(topic);

    if !out.json {
        writeln!(
            out.w,
            "ac2: following {name} ({topic}) for {} s (Ctrl-C ends early); {AGE_NOTE}",
            a.seconds
        )?;
    }
    c.subscribe(sub)?;
    let start = Instant::now();
    let end = start + Duration::from_secs_f64(a.seconds);
    let mut tracker = Tracker::new(start, rate_hz);
    let mut total = Record::starting(0.0);
    let mut window = Record::starting(0.0);
    let mut latest = Latest::default();
    let mut tick = tokio::time::interval_at((start + REPORT_EVERY).into(), REPORT_EVERY);
    let quit = quit_signal();
    tokio::pin!(quit);
    let result: Result<(), CliError> = loop {
        tokio::select! {
            () = &mut quit => break Ok(()),
            () = tokio::time::sleep_until(end.into()) => break Ok(()),
            silent = c.await_dead() => break Err(dead(silent)),
            _ = tick.tick(), if !out.json => {
                let now_s = start.elapsed().as_secs_f64();
                writeln!(out.w, "{}", line(&window.summary(now_s), now_s))?;
                out.w.flush()?;
                window = Record::starting(now_s);
            }
            () = c.data_changed() => {
                if let Err(e) = c.latest_into(&mut latest) {
                    break Err(e.into());
                }
                for arr in latest.arrivals.iter().filter(|x| x.topic == topic) {
                    let row = tracker.row(arr);
                    total.push(&row);
                    window.push(&row);
                    if out.json {
                        out.json_line(&serde_json::json!({ "frame": row }))?;
                    }
                }
            }
        }
    };
    let _ = c.unsubscribe(sub);
    result?;
    let to_s = start.elapsed().as_secs_f64().min(a.seconds);
    let summary = total.summary(to_s);
    let closing = Closing {
        summary: &summary,
        meas: &name,
        topic: topic.to_string(),
        rate_hz,
        clock_offset_ms: c.view().clock_offset_ns.map(|o| o as f64 / 1e6),
        note: AGE_NOTE,
    };
    if out.json {
        out.json_line(&closing)?;
    } else {
        write!(out.w, "{}", text(&closing))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use super::*;
    use ac2_proto::Stream;
    use ac2_proto::frame::{FrameStamp, ProtectionFlags};
    use ac2_proto::units::{DaemonIncarnation, MeasId, Rev, SampleIndex, SessionEpoch, WallNs};

    const RATE: u32 = 48_000;

    fn arrival(start: Instant, t_ms: u64, seq: u64, stamp: Option<FrameStamp>) -> Arrival {
        Arrival {
            topic: Topic::Data {
                meas: MeasId(1),
                stream: Stream::Spec,
            },
            seq,
            keyframe: false,
            stamp,
            bytes: 1000,
            at: start + Duration::from_millis(t_ms),
            wall_ns: 1_000_000_000_000 + i128::from(t_ms) * 1_000_000,
        }
    }

    fn stamp(seq: u64, capture_ms: u64) -> FrameStamp {
        FrameStamp {
            seq,
            audio_sample: SampleIndex(seq * 960),
            session_epoch: SessionEpoch(1),
            daemon_incarnation: DaemonIncarnation(1),
            config_rev: Rev(1),
            config_applied_at: SampleIndex(0),
            capture_wall_ns: WallNs(1_000_000_000_000 + capture_ms * 1_000_000),
            grid_id: None,
            protection: ProtectionFlags::NONE,
        }
    }

    /// 20 ms hops arriving every 21 ms (the age grows 1 ms a frame), frame 5 lost and frame
    /// 6 a further 10 ms late (one gap over twice the median), then a repeat and a refused
    /// delta, which count nowhere but in their own tallies.
    #[test]
    fn rows_and_summary() {
        let start = Instant::now();
        let mut tr = Tracker::new(start, Some(RATE));
        let mut rec = Record::starting(0.0);
        let mut rows = Vec::new();
        for seq in 0..10u64 {
            if seq == 5 {
                continue;
            }
            let t = seq * 21 + 10 + if seq == 6 { 10 } else { 0 };
            rows.push(tr.row(&arrival(start, t, seq, Some(stamp(seq, seq * 20)))));
        }
        rows.push(tr.row(&arrival(start, 200, 9, Some(stamp(9, 180)))));
        rows.push(tr.row(&arrival(start, 201, 10, None)));
        for r in &rows {
            rec.push(r);
        }
        assert_eq!(rows[0].interval_ms, None);
        assert_eq!(rows[1].hop_ms, Some(20.0));
        assert_eq!(rows[1].age_ms, Some(11.0));
        assert_eq!(rows[5].seq_missing, 1);
        assert_eq!(rows[5].hop_ms, Some(40.0));
        assert_eq!(rows[5].age_ms, Some(26.0));
        assert!(rows[9].repeat);
        assert!(!rows[10].applied);

        let s = rec.summary(1.0);
        assert_eq!((s.frames, s.repeats, s.unapplied), (9, 1, 1));
        assert_eq!((s.seq_missing, s.seq_gap_events), (1, 1));
        let i = s.interval_ms.clone().unwrap();
        assert!((i.p50 - 21.0).abs() < 1e-6, "{i:?}");
        assert!((i.max - 52.0).abs() < 1e-6, "{i:?}");
        assert_eq!(s.gaps_over_2x_median, 1);
        let a = s.age_ms.clone().unwrap();
        assert_eq!((a.min, a.p50, a.max), (10.0, 14.0, 26.0));
        // The least-squares line through the ages over their arrival times (worked out
        // apart): about 1 ms per 17 ms, the late frame pulling it up.
        let d = s.age_drift_ms_per_min.unwrap();
        assert!((d - 3498.889).abs() < 0.01, "{d}");
        assert!((s.mbit_s - 11.0 * 1000.0 * 8.0 / 1e6).abs() < 1e-9);
        assert!(line(&s, 1.0).contains("9 frames"));
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        let s = spread(&v).unwrap();
        assert_eq!((s.p50, s.p95, s.p99, s.max), (50.0, 95.0, 99.0, 100.0));
        assert!(spread(&[]).is_none());
        assert_eq!(pct(&[3.0], 99.0), 3.0);
    }
}
