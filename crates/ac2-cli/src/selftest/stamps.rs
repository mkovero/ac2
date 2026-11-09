//! `ac2 selftest stamps`: a capture-only log of what a backend says about time, for the
//! clock study (`docs/design/timing-study.md` §5). The stream opens no outputs at all, so
//! nothing can sound; on JACK it is a client of its own beside a running daemon.
//!
//! The log directory holds:
//!
//! - `blocks.bin`: every capture block, taken from the consumer side of the block ring (the
//!   audio callback only fills the ring, as for any stream). A 16-byte header, then one
//!   32-byte record per block, all little-endian:
//!
//!   ```text
//!   header  0  magic "AC2STMP1"   8  u32 format version (1)   12  u32 record bytes (32)
//!   record  0  u64 start_sample   8  u32 frames   12  u32 flags (ac2-audio BlockFlags bits)
//!          16  u64 capture_ns (u64::MAX: the backend gave none)   24  u64 callback_ns
//!   ```
//!
//!   Both stamps are on the stream's own stamp clock (JACK: `jack_get_time`). An hour at
//!   96 kHz / 128 frames is 2.7 M records, 86 MB: fixed records cost a numpy reader one
//!   `fromfile`, and the volume does not justify a codec.
//! - `pairs.csv`: about once a second, the stamp clock read inside `CLOCK_MONOTONIC` reads,
//!   those inside `CLOCK_REALTIME` reads, with `CLOCK_MONOTONIC_RAW` right after the stamp
//!   (Linux only; elsewhere the file has its header alone). A read whose monotonic bracket
//!   is wider than [`MAX_BRACKET_NS`] was preempted: it is counted, not written.
//! - `meta.json`: host, backend, device, rate, buffer, inputs, the backend's latency
//!   statement (JACK: the capture ports' latency range, min and max; the block stamps use
//!   the max), ac2 build, start time, the stamp clock as the backend names it and which host
//!   clock it reads equal to; rewritten at the end with the counts.
//! - `keep.wav` (with `--keep`): that input's samples as delivered, f32 (RF64 past 4 GiB).
//!   Its first frame is `keep.first_start_sample` in `meta.json`; frames lost to a gap are
//!   absent from the file, and `blocks.bin` says where they were.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ac2_audio::{
    Backend, BackendKind, BlockFlags, BlockHeader, DeviceSelector, DuplexRequest, DuplexStream,
    EventSnapshot, MaxLevel, StampClock, StaticLatency, TransportStats,
};
use ac2_core::clock_rate::RateFit;
use ac2_traces::raw::WavWriter;
use serde::Serialize;

use super::stats::StreamStats;
use super::{CaptureStats, ClockRate};

/// `blocks.bin` magic.
pub const MAGIC: &[u8; 8] = b"AC2STMP1";
/// `blocks.bin` format version.
pub const FORMAT_VERSION: u32 = 1;
/// Bytes per `blocks.bin` record.
pub const RECORD_BYTES: usize = 32;
/// Bytes before the first record.
pub const HEADER_BYTES: usize = 16;
/// `capture_ns` of a block the backend did not stamp.
pub const NO_CAPTURE_NS: u64 = u64::MAX;

/// Widest monotonic bracket a kept pair may have: back-to-back reads lie a few hundred ns
/// apart, more means the thread was preempted between them (the daemon's rule).
pub const MAX_BRACKET_NS: u64 = 20_000;

/// Pairs are read this often: a minute of them pins the stamp clock against the host
/// clocks to well under 0.1 ppm, and an hour stays a small file.
const PAIR_EVERY: Duration = Duration::from_secs(1);

/// The ring holds this much audio: file writes on a slow disk can stall the reader for a
/// while, and a lost block is a gap in the very record this tool exists to make.
const RING_SECONDS: f64 = 10.0;

/// The `pairs.csv` header.
pub const PAIRS_HEADER: &str = "stamp_ns,monotonic_ns,monotonic_bracket_ns,realtime_ns,\
realtime_bracket_ns,monotonic_raw_ns";

/// The stream to log.
#[derive(Clone, Debug, PartialEq)]
pub struct StampsSpec {
    /// Capture device.
    pub input_device: DeviceSelector,
    /// Zero-based device inputs to capture.
    pub input_map: Vec<u16>,
    /// Sample rate; `None` takes the device's default.
    pub sample_rate: Option<u32>,
    /// Callback size; `None` takes the backend's choice.
    pub buffer_frames: Option<u32>,
    /// Zero-based device input whose samples are kept; must be captured.
    pub keep: Option<u16>,
}

/// Who made the log, for `meta.json`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Provenance {
    /// Host name.
    pub host: String,
    /// ac2 build id.
    pub ac2_build: String,
}

/// Why the log could not start or finish.
#[derive(Debug)]
pub enum StampsError {
    /// The kept input is not among the captured inputs.
    KeepNotCaptured(u16),
    /// The output directory already holds a log.
    Exists(PathBuf),
    /// The backend refused the stream.
    Open(String),
    /// A file could not be written.
    Io(io::Error),
}

impl std::fmt::Display for StampsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeepNotCaptured(c) => {
                write!(f, "--keep {}: not among the captured inputs", c + 1)
            }
            Self::Exists(p) => write!(f, "{} already holds a log", p.display()),
            Self::Open(m) => write!(f, "cannot open the stream: {m}"),
            Self::Io(e) => write!(f, "cannot write the log: {e}"),
        }
    }
}

impl std::error::Error for StampsError {}

impl From<io::Error> for StampsError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// The line through `capture_ns` against `start_sample`, and how far the stamps lie off it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct StampFit {
    /// Blocks with a capture stamp.
    pub points: u64,
    /// Contiguous stretches fitted (each about its own mean, slope shared).
    pub segments: u64,
    /// The device clock against the stamp clock.
    pub rate: Option<ClockRate>,
    /// Standard deviation of the stamps about the line, ns.
    pub residual_sigma_ns: Option<f64>,
}

/// The kept channel.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Kept {
    /// One-based device input.
    pub channel: u16,
    /// File name in the log directory.
    pub file: String,
    /// `start_sample` of the file's first frame; `None` until a block arrived.
    pub first_start_sample: Option<u64>,
    /// Frames in the file.
    pub frames: u64,
}

/// The paired clock reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct PairCounts {
    /// Written to `pairs.csv`.
    pub kept: u64,
    /// Read with a monotonic bracket over [`MAX_BRACKET_NS`], not written.
    pub dropped: u64,
    /// The stamp clock could not be read, or this OS has no host clocks to pair it with.
    pub unavailable: u64,
}

/// `meta.json` and the run's summary.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StampsMeta {
    /// `blocks.bin` format version.
    pub format_version: u32,
    /// Host and build.
    #[serde(flatten)]
    pub provenance: Provenance,
    /// Backend.
    pub backend: BackendKind,
    /// Capture device id.
    pub device: String,
    /// Stream rate, Hz.
    pub sample_rate: u32,
    /// Callback size when fixed by the host.
    pub buffer_frames: Option<u32>,
    /// Captured inputs, one-based device channels.
    pub inputs: Vec<u16>,
    /// The backend's latency statement as it gives it.
    pub latency: StaticLatency,
    /// Capture latency range, frames, `[min, max]`, where the backend states one.
    pub capture_latency_frames: Option<[u32; 2]>,
    /// Start, ns since the Unix epoch (`CLOCK_REALTIME`).
    pub start_realtime_ns: u64,
    /// Start, UTC.
    pub start_utc: String,
    /// The stamp clock as the backend names it; `None`: the stream has none to read.
    pub stamp_clock: Option<String>,
    /// The host clock the stamp clock reads equal to, from the last kept pair:
    /// `CLOCK_MONOTONIC_RAW`, `CLOCK_MONOTONIC`, or `None` (neither, or not yet told apart).
    pub stamp_clock_reads_as: Option<&'static str>,
    /// The kept channel.
    pub keep: Option<Kept>,
    /// Filled in when the run ends.
    pub end: Option<StampsEnd>,
}

/// How the run went.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StampsEnd {
    /// Ctrl-C ended it early.
    pub interrupted: bool,
    /// Wall-clock seconds the stream ran.
    pub seconds: f64,
    /// Block accounting (counts, gaps, xruns, intervals).
    pub capture: CaptureStats,
    /// The fit of the capture stamps.
    pub fit: StampFit,
    /// Paired clock reads.
    pub pairs: PairCounts,
    /// What the host reported.
    pub host_events: EventSnapshot,
    /// Ring transport.
    pub transport: TransportStats,
}

/// A running log.
struct Logger {
    dir: PathBuf,
    stream: DuplexStream,
    clock: Option<Arc<dyn StampClock>>,
    blocks: BufWriter<File>,
    pairs: BufWriter<File>,
    wav: Option<(usize, WavWriter)>,
    meta: StampsMeta,
    stats: StreamStats,
    counts: PairCounts,
    buf: Vec<f32>,
    keep_buf: Vec<f32>,
    record: [u8; RECORD_BYTES],
}

fn create(dir: &Path, name: &str) -> Result<File, StampsError> {
    let p = dir.join(name);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => StampsError::Exists(dir.to_path_buf()),
            _ => StampsError::Io(e),
        })
}

fn realtime_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

impl Logger {
    fn open(
        backend: &dyn Backend,
        spec: &StampsSpec,
        dir: &Path,
        provenance: Provenance,
    ) -> Result<Self, StampsError> {
        let keep_idx = match spec.keep {
            Some(k) => Some(
                spec.input_map
                    .iter()
                    .position(|&c| c == k)
                    .ok_or(StampsError::KeepNotCaptured(k))?,
            ),
            None => None,
        };
        fs::create_dir_all(dir)?;
        let mut blocks = BufWriter::new(create(dir, "blocks.bin")?);
        let mut pairs = BufWriter::new(create(dir, "pairs.csv")?);
        // No outputs at all: the output side of the request is empty, so no output port or
        // device is opened and the peak limit below never meets a sample.
        let max_level =
            MaxLevel::from_peak_db(ac2_core::generator::peak_limit_db(super::CEILING_DBFS))
                .map_err(|e| StampsError::Open(e.to_string()))?;
        let mut request = DuplexRequest::new(spec.input_map.clone(), 0, max_level);
        request.input_device = spec.input_device.clone();
        request.sample_rate = spec.sample_rate;
        request.buffer_frames = spec.buffer_frames;
        request.ring_seconds = RING_SECONDS;
        let stream = backend
            .open(request)
            .map_err(|e| StampsError::Open(e.to_string()))?;
        let n = stream.negotiated().clone();
        let start_realtime_ns = realtime_ns();
        let clock = stream.stamp_clock();
        let wav = match (keep_idx, spec.keep) {
            (Some(idx), Some(_)) => {
                let path = dir.join("keep.wav");
                if path.exists() {
                    return Err(StampsError::Exists(dir.to_path_buf()));
                }
                Some((idx, WavWriter::create(&path, 1, n.sample_rate)?))
            }
            _ => None,
        };
        let capture_latency_frames = match n.latency {
            StaticLatency::PortRanges { capture, .. } => Some([capture.min, capture.max]),
            _ => None,
        };
        let meta = StampsMeta {
            format_version: FORMAT_VERSION,
            provenance,
            backend: n.backend,
            device: n.input_device.0.clone(),
            sample_rate: n.sample_rate,
            buffer_frames: n.buffer_frames,
            inputs: spec.input_map.iter().map(|c| c + 1).collect(),
            latency: n.latency.clone(),
            capture_latency_frames,
            start_realtime_ns,
            start_utc: chrono::DateTime::<chrono::Utc>::from(SystemTime::now())
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            stamp_clock: clock.as_ref().map(|c| format!("{c:?}")),
            stamp_clock_reads_as: None,
            keep: spec.keep.map(|k| Kept {
                channel: k + 1,
                file: "keep.wav".into(),
                first_start_sample: None,
                frames: 0,
            }),
            end: None,
        };
        let mut header = [0u8; HEADER_BYTES];
        header[..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        header[12..16].copy_from_slice(&(RECORD_BYTES as u32).to_le_bytes());
        blocks.write_all(&header)?;
        writeln!(pairs, "{PAIRS_HEADER}")?;
        let logger = Self {
            dir: dir.to_path_buf(),
            stats: StreamStats::new(n.sample_rate, n.input_channels),
            stream,
            clock,
            blocks,
            pairs,
            wav,
            meta,
            counts: PairCounts::default(),
            buf: Vec::new(),
            keep_buf: Vec::new(),
            record: [0; RECORD_BYTES],
        };
        // Written now so that a run killed outright still says what it logged.
        logger.write_meta()?;
        Ok(logger)
    }

    fn write_meta(&self) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(&self.meta).map_err(io::Error::other)?;
        fs::write(self.dir.join("meta.json"), json)
    }

    /// Takes every waiting block; returns whether there was any.
    fn poll(&mut self) -> io::Result<bool> {
        let mut any = false;
        while let Some(h) = self.stream.capture().pop_into(&mut self.buf) {
            self.stats.add_block(&h, &self.buf);
            encode(&h, &mut self.record);
            self.blocks.write_all(&self.record)?;
            if let Some((idx, w)) = &mut self.wav {
                let ch = usize::from(h.channels.max(1));
                self.keep_buf.clear();
                self.keep_buf
                    .extend(self.buf.iter().skip(*idx).step_by(ch).copied());
                w.write(&self.keep_buf)?;
                if let Some(k) = &mut self.meta.keep {
                    k.first_start_sample.get_or_insert(h.start_sample);
                    k.frames = w.frames();
                }
            }
            any = true;
        }
        Ok(any)
    }

    /// One paired read of the stamp clock against the host clocks.
    fn pair(&mut self) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        if let Some(h) = self
            .clock
            .as_deref()
            .and_then(ac2_audio::bracket_host_clocks)
        {
            if h.monotonic_bracket_ns > MAX_BRACKET_NS {
                self.counts.dropped += 1;
                return Ok(());
            }
            self.counts.kept += 1;
            self.meta.stamp_clock_reads_as = reads_as(&h);
            return writeln!(
                self.pairs,
                "{},{},{},{},{},{}",
                h.stamp_ns,
                h.monotonic_ns,
                h.monotonic_bracket_ns,
                h.realtime_ns,
                h.realtime_bracket_ns,
                h.monotonic_raw_ns
            );
        }
        // Only Linux has the host clocks read here; elsewhere every read is unavailable.
        #[cfg(not(target_os = "linux"))]
        let _ = &self.clock;
        self.counts.unavailable += 1;
        Ok(())
    }

    fn finish(mut self, interrupted: bool, seconds: f64) -> Result<StampsMeta, StampsError> {
        self.poll()?;
        let events = self.stream.events();
        let transport = self.stream.transport_stats();
        // Nothing was emitted, so the stop does not wait for a fade.
        self.stream.stop(DuplexStream::DEFAULT_STOP_TIMEOUT);
        let rate = f64::from(self.meta.sample_rate);
        let mut blocks = self.blocks.into_inner().map_err(|e| e.into_error())?;
        blocks.flush()?;
        blocks.sync_all()?;
        let mut pairs = self.pairs.into_inner().map_err(|e| e.into_error())?;
        pairs.flush()?;
        pairs.sync_all()?;
        if let Some((_, w)) = self.wav.take() {
            let info = w.finish()?;
            if let Some(k) = &mut self.meta.keep {
                k.frames = info.frames;
            }
        }
        let fit = fit_file(&self.dir.join("blocks.bin"), rate)?;
        let (capture, _) = self.stats.finish();
        self.meta.end = Some(StampsEnd {
            interrupted,
            seconds,
            capture,
            fit,
            pairs: self.counts,
            host_events: events,
            transport,
        });
        let json = serde_json::to_vec_pretty(&self.meta).map_err(io::Error::other)?;
        fs::write(self.dir.join("meta.json"), json)?;
        Ok(self.meta)
    }
}

/// The host clock the stamp clock reads equal to: the stamp within the bracket of one
/// clock and clearly off the other. Right after boot the two monotonic clocks have not yet
/// drifted apart by more than the bracket, and the read cannot tell them apart.
#[cfg(target_os = "linux")]
fn reads_as(h: &ac2_audio::HostClocks) -> Option<&'static str> {
    let tol = h.monotonic_bracket_ns + 1_000;
    let raw = h.monotonic_raw_ns.abs_diff(h.stamp_ns) <= tol;
    let mono = h.monotonic_ns.abs_diff(h.stamp_ns) <= tol;
    match (raw, mono) {
        (true, false) => Some("CLOCK_MONOTONIC_RAW"),
        (false, true) => Some("CLOCK_MONOTONIC"),
        _ => None,
    }
}

/// `h` as a `blocks.bin` record.
pub fn encode(h: &BlockHeader, out: &mut [u8; RECORD_BYTES]) {
    out[0..8].copy_from_slice(&h.start_sample.to_le_bytes());
    out[8..12].copy_from_slice(&h.frames.to_le_bytes());
    out[12..16].copy_from_slice(&h.flags.bits().to_le_bytes());
    out[16..24].copy_from_slice(&h.capture_ns.unwrap_or(NO_CAPTURE_NS).to_le_bytes());
    out[24..32].copy_from_slice(&h.callback_ns.to_le_bytes());
}

/// One `blocks.bin` record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    /// First frame's index.
    pub start_sample: u64,
    /// Frames.
    pub frames: u32,
    /// Block flags.
    pub flags: BlockFlags,
    /// Capture stamp, if the backend gave one.
    pub capture_ns: Option<u64>,
    /// Callback stamp.
    pub callback_ns: u64,
}

fn decode(b: &[u8; RECORD_BYTES]) -> Record {
    let u64_at = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().expect("8 bytes"));
    let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().expect("4 bytes"));
    let cap = u64_at(16);
    Record {
        start_sample: u64_at(0),
        frames: u32_at(8),
        flags: BlockFlags::from_bits_truncate(u32_at(12)),
        capture_ns: (cap != NO_CAPTURE_NS).then_some(cap),
        callback_ns: u64_at(24),
    }
}

/// Reads `blocks.bin`, calling `f` per record.
pub fn read_blocks(path: &Path, mut f: impl FnMut(Record)) -> io::Result<()> {
    let mut r = BufReader::new(File::open(path)?);
    let mut header = [0u8; HEADER_BYTES];
    r.read_exact(&mut header)?;
    if &header[..8] != MAGIC
        || header[8..12] != FORMAT_VERSION.to_le_bytes()
        || header[12..16] != (RECORD_BYTES as u32).to_le_bytes()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a blocks.bin of this format",
        ));
    }
    let mut rec = [0u8; RECORD_BYTES];
    loop {
        match r.read_exact(&mut rec) {
            Ok(()) => f(decode(&rec)),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}

/// Whether `r` starts a new stretch after `prev`: a flagged break, or an index that does
/// not follow on (frames lost without a flag still move the line's origin).
fn breaks(prev: Option<&Record>, r: &Record) -> bool {
    r.flags.breaks_continuity()
        || prev.is_some_and(|p| p.start_sample + u64::from(p.frames) != r.start_sample)
}

/// The capture-stamp line of `blocks.bin` at `nominal` Hz: the pooled slope over every
/// stretch, then the scatter about it, each stretch about its own mean. Two passes over
/// the file: an hour's stamps squared would not survive in one f64 sum.
pub fn fit_file(path: &Path, nominal: f64) -> io::Result<StampFit> {
    let mut fit = RateFit::default();
    let mut prev: Option<Record> = None;
    read_blocks(path, |r| {
        if let Some(ns) = r.capture_ns {
            fit.add(r.start_sample, ns, breaks(prev.as_ref(), &r));
        }
        prev = Some(r);
    })?;
    let rate = ClockRate::from_fit(fit, nominal);
    let mut out = StampFit {
        points: 0,
        segments: 0,
        rate,
        residual_sigma_ns: None,
    };
    let Some(r) = rate else {
        return Ok(out);
    };
    let ns_per_sample = 1e9 / r.hz;
    // Per stretch: origin sample and stamp, n, Σr, Σr².
    let mut seg: Option<(u64, u64, f64, f64, f64)> = None;
    let mut ss = 0.0;
    let mut prev: Option<Record> = None;
    let close = |(_, _, n, sr, srr): (u64, u64, f64, f64, f64), ss: &mut f64| {
        *ss += srr - sr * sr / n;
    };
    read_blocks(path, |rec| {
        let brk = breaks(prev.as_ref(), &rec);
        prev = Some(rec);
        let Some(ns) = rec.capture_ns else { return };
        out.points += 1;
        if brk && let Some(s) = seg.take() {
            close(s, &mut ss);
        }
        let s = seg.get_or_insert_with(|| {
            out.segments += 1;
            (rec.start_sample, ns, 0.0, 0.0, 0.0)
        });
        let x = rec.start_sample.wrapping_sub(s.0) as i64 as f64;
        let y = ns.wrapping_sub(s.1) as i64 as f64;
        let res = y - x * ns_per_sample;
        s.2 += 1.0;
        s.3 += res;
        s.4 += res * res;
    })?;
    if let Some(s) = seg.take() {
        close(s, &mut ss);
    }
    // One mean per stretch and the shared slope are fitted parameters.
    let dof = out.points as f64 - out.segments as f64 - 1.0;
    out.residual_sigma_ns = (dof > 0.0).then(|| (ss.max(0.0) / dof).sqrt());
    Ok(out)
}

/// Logs `spec` into `dir` for `duration` of wall-clock time, or until `stop` is set
/// (reported as interrupted), then closes every file cleanly.
pub fn run(
    backend: &dyn Backend,
    spec: &StampsSpec,
    dir: &Path,
    provenance: Provenance,
    duration: Duration,
    stop: &AtomicBool,
) -> Result<StampsMeta, StampsError> {
    let mut log = Logger::open(backend, spec, dir, provenance)?;
    let started = Instant::now();
    let end = started + duration;
    let mut next_pair = started;
    let mut interrupted = false;
    loop {
        let now = Instant::now();
        if now >= end {
            break;
        }
        if stop.load(Ordering::Acquire) {
            interrupted = true;
            break;
        }
        if now >= next_pair {
            log.pair()?;
            next_pair += PAIR_EVERY;
        }
        if !log.poll()? {
            // The ring holds seconds of audio: a few milliseconds' nap loses nothing.
            std::thread::sleep(Duration::from_millis(3));
        }
    }
    log.finish(interrupted, started.elapsed().as_secs_f64())
}

#[cfg(test)]
#[path = "stamps_tests.rs"]
mod tests;
