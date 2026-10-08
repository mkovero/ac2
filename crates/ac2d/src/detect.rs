//! Loopback detection: a short noise burst on one output of a device, then which input of
//! the same or another device it came back on.
//!
//! Audio safety: the burst is built in full before the stream opens — a band-limited pink
//! noise of [`BURST_S`] at the operator's typed level, refused above the global ceiling,
//! faded in and out by the generator's 20 ms ramp — and played by a source that can only
//! emit those samples once and then zeros. The stream's output path enforces the global
//! peak limit and fades every change of audibility. The burst is routed to the one output
//! asked for; every other output of the stream stays silent.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_audio::{
    Backend, DeviceSelector, DuplexRequest, Gain, HistoryRequest, MaxLevel, OutputSource,
    SignalSource, generator,
};
use ac2_core::generator::{
    BandLimit, FilterOrder, Generator as CoreGenerator, GeneratorConfig, Signal, dbfs_to_rms,
};
use ac2_core::loopback::{LOOPBACK_MIN_CORRELATION, rank_inputs};
use ac2_proto::ErrorCode;
use ac2_proto::ProtoError;
use ac2_proto::model::{BackendKind, DeviceId, LoopbackCandidate, LoopbackDetection};
use ac2_proto::units::{Db, Dbfs, Samples, Seconds};

use crate::conv;
use crate::fanout::pop_block;
use crate::session::audio_err;
use crate::util::{perr, random_u64};

/// Burst length before the fade-out, s.
pub(crate) const BURST_S: f64 = 0.5;
/// Longest return delay searched, s (a loopback returns within milliseconds; a mic a few
/// tens of metres away within a tenth of a second).
const MAX_DELAY_S: f64 = 0.5;
/// Capture kept before the burst's first sample, s: on a clock with separate callbacks the
/// capture and output counters are offset, so a return may index slightly "early".
const PRE_S: f64 = 0.1;
/// Audio captured before the burst starts, s (the stream is running and settled).
const WARMUP_S: f64 = 0.1;
/// Band of the burst: wide enough for a sharp correlation peak, clear of the subsonic
/// rumble and the top octave a loopback may not pass.
const BURST_HP_HZ: f64 = 100.0;
const BURST_LP_HZ: f64 = 10_000.0;
/// Longest the whole detection may take before the device is declared silent.
const TIMEOUT: Duration = Duration::from_secs(6);
/// Most inputs searched.
const MAX_INPUTS: u16 = 64;
const STOP_TIMEOUT: Duration = Duration::from_millis(300);

/// One detection, validated by control.
pub(crate) struct DetectRequest {
    pub(crate) backend: Arc<dyn Backend>,
    pub(crate) kind: BackendKind,
    pub(crate) input_device: DeviceId,
    pub(crate) output_device: DeviceId,
    pub(crate) output: u16,
    pub(crate) level_dbfs: f64,
    pub(crate) ceiling_dbfs: f64,
    pub(crate) max_level: MaxLevel,
}

/// Plays a fixed buffer once, then zeros. Allocation-free.
struct Burst {
    samples: Box<[f32]>,
    pos: usize,
}

impl SignalSource for Burst {
    fn fill(&mut self, out: &mut [f32]) {
        for o in out {
            *o = self.samples.get(self.pos).copied().unwrap_or(0.0);
            self.pos = (self.pos + 1).min(self.samples.len());
        }
    }
}

/// The burst at `level_dbfs` for `fs`: [`BURST_S`] of band-limited pink noise with the
/// generator's ramp in and a ramp out.
fn burst(
    fs: u32,
    level_dbfs: f64,
    ceiling_dbfs: f64,
    max_level: MaxLevel,
) -> Result<Vec<f32>, ProtoError> {
    let fs_f = f64::from(fs);
    let mut g = CoreGenerator::new(&GeneratorConfig {
        signal: Signal::Pink,
        sample_rate: fs_f,
        seed: random_u64(),
        band: BandLimit {
            highpass_hz: Some(BURST_HP_HZ),
            lowpass_hz: Some(BURST_LP_HZ.min(0.45 * fs_f)),
            order: FilterOrder::Fourth,
        },
        level_dbfs,
        ceiling_dbfs,
    })
    .map_err(|e| perr(ErrorCode::Refused, format!("burst level: {e}")))?;
    let peak = dbfs_to_rms(level_dbfs) * g.crest_factor();
    if !max_level.admits_peak(peak) {
        return Err(perr(
            ErrorCode::Refused,
            "the burst's peak would exceed the output limit",
        ));
    }
    let on = (BURST_S * fs_f).round() as usize;
    let ramp = g.ramp_samples() as usize;
    let mut out = vec![0.0f32; on + ramp];
    g.fill(&mut out[..on]);
    g.level_control().fade_out();
    g.fill(&mut out[on..]);
    Ok(out)
}

/// Captured channels from capture index `start`, gaps zero-filled.
struct Capture {
    start: Option<u64>,
    channels: Vec<Vec<f32>>,
}

impl Capture {
    fn end(&self) -> u64 {
        self.start.unwrap_or(0) + self.channels.first().map_or(0, |c| c.len() as u64)
    }

    fn push(&mut self, b: &crate::fanout::Block) -> Result<(), ProtoError> {
        let start = *self.start.get_or_insert(b.start_sample);
        let at = b.start_sample;
        let have = self.end();
        if at < have || at < start {
            return Err(perr(
                ErrorCode::Internal,
                "the capture restarted during the burst; detect again",
            ));
        }
        let n = usize::from(b.channels).max(1);
        let gap = (at - have) as usize;
        for (ch, v) in self.channels.iter_mut().enumerate() {
            v.extend(std::iter::repeat_n(0.0, gap));
            v.extend(b.data.iter().skip(ch).step_by(n).copied());
        }
        Ok(())
    }

    /// Channel `ch` over capture indices `from .. to` (zeros outside what was captured).
    fn range(&self, ch: usize, from: u64, to: u64) -> Vec<f64> {
        let s = self.start.unwrap_or(0);
        (from..to)
            .map(|i| {
                i.checked_sub(s)
                    .and_then(|k| self.channels[ch].get(k as usize))
                    .map_or(0.0, |v| f64::from(*v))
            })
            .collect()
    }
}

/// Runs one detection; blocks for about a second.
pub(crate) fn run(r: &DetectRequest) -> Result<LoopbackDetection, ProtoError> {
    let listed = r.backend.enumerate().map_err(audio_err)?;
    let find = |id: &DeviceId| {
        listed.iter().find(|d| d.id.0 == id.0).ok_or_else(|| {
            perr(
                ErrorCode::NotFound,
                format!("no {:?} device {:?}", r.kind, id.0),
            )
        })
    };
    let in_caps = find(&r.input_device)?;
    let out_caps = find(&r.output_device)?;
    let inputs = in_caps
        .input
        .as_ref()
        .map_or(0, |i| i.max_channels)
        .min(MAX_INPUTS);
    let outputs = out_caps.output.as_ref().map_or(0, |o| o.max_channels);
    if inputs == 0 {
        return Err(perr(
            ErrorCode::Invalid,
            format!("{} has no inputs to listen on", in_caps.name),
        ));
    }
    if r.output >= outputs {
        return Err(perr(
            ErrorCode::Invalid,
            format!(
                "output {} does not exist: {} has {outputs} outputs",
                r.output + 1,
                out_caps.name
            ),
        ));
    }
    let (mut handle, port) =
        generator(vec![r.output]).map_err(|e| perr(ErrorCode::Invalid, e.to_string()))?;
    let selector = |id: &DeviceId| DeviceSelector::Id(ac2_audio::DeviceId(id.0.clone()));
    let mut req = DuplexRequest::new((0..inputs).collect(), r.output + 1, r.max_level);
    req.input_device = selector(&r.input_device);
    req.output_device = selector(&r.output_device);
    req.output = OutputSource::Generator(port);
    req.history = Some(HistoryRequest::channel(r.output));
    let mut stream = r.backend.open(req).map_err(audio_err)?;
    let n = stream.negotiated().clone();
    let fs = n.sample_rate;
    let history = stream
        .history()
        .cloned()
        .ok_or_else(|| perr(ErrorCode::Internal, "no generator history"))?;
    let result = (|| {
        let samples = burst(fs, r.level_dbfs, r.ceiling_dbfs, r.max_level)?;
        let len = samples.len() as u64;
        let fs_f = f64::from(fs);
        let max_lag = (MAX_DELAY_S * fs_f).round() as u64;
        let pre = (PRE_S * fs_f).round() as u64;
        let warmup = (WARMUP_S * fs_f).round() as u64;
        let t0 = Instant::now();
        let mut cap = Capture {
            start: None,
            channels: vec![Vec::new(); usize::from(n.input_channels)],
        };
        let mut pump = |cap: &mut Capture| -> Result<(), ProtoError> {
            if t0.elapsed() > TIMEOUT {
                return Err(perr(
                    ErrorCode::Internal,
                    "the device delivered no audio; is it running?",
                ));
            }
            let mut got = false;
            while let Some(b) = pop_block(&mut stream) {
                cap.push(&b)?;
                got = true;
            }
            if !got {
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        };
        while cap.end() < cap.start.unwrap_or(u64::MAX).saturating_add(warmup) {
            pump(&mut cap)?;
        }
        let searched_from = history.written_end();
        handle
            .set_source(Box::new(Burst {
                samples: samples.into_boxed_slice(),
                pos: 0,
            }))
            .map_err(|e| perr(ErrorCode::Internal, e.to_string()))?;
        handle.set_gain(Gain::UNITY);
        handle.start();
        tracing::info!(
            target: "ac2d::audit",
            "loopback burst {:.1} dBFS on output {} of {:?} {:?}",
            r.level_dbfs,
            r.output + 1,
            r.kind,
            r.output_device.0
        );
        // The first emitted sample, found in the history of what was actually rendered.
        let mut first: Option<u64> = None;
        let mut scanned = searched_from;
        let mut buf = Vec::new();
        loop {
            pump(&mut cap)?;
            let end = history.written_end();
            if first.is_none() && end > scanned {
                let from = scanned.max(end.saturating_sub(history.capacity()));
                buf.resize((end - from) as usize, 0.0);
                if history.read(from, &mut buf).is_ok() {
                    first = buf.iter().position(|v| *v != 0.0).map(|i| from + i as u64);
                    scanned = end;
                }
            }
            if let Some(f) = first
                && end >= f + len
                && cap.end() >= f + len + max_lag
            {
                break;
            }
        }
        handle.stop();
        let first = first.unwrap_or(0);
        let mut x = vec![0.0f32; len as usize];
        history
            .read(first, &mut x)
            .map_err(|e| perr(ErrorCode::Internal, format!("burst history: {e}")))?;
        let x: Vec<f64> = x.iter().map(|v| f64::from(*v)).collect();
        let pre = pre.min(first.saturating_sub(cap.start.unwrap_or(0)));
        let from = first - pre;
        let to = first + len + max_lag;
        let ys: Vec<Vec<f64>> = (0..cap.channels.len())
            .map(|ch| cap.range(ch, from, to))
            .collect();
        let refs: Vec<&[f64]> = ys.iter().map(Vec::as_slice).collect();
        let ranked: Vec<LoopbackCandidate> = rank_inputs(&x, &refs, (max_lag + pre) as usize)
            .into_iter()
            .map(|c| {
                let d = c.lag as i64 - pre as i64;
                LoopbackCandidate {
                    input: c.input as u16,
                    delay: Seconds(d as f64 / fs_f),
                    delay_samples: Samples(d),
                    correlation: c.correlation,
                    gain: c.gain_db.map(Db),
                }
            })
            .collect();
        let loopback = ranked
            .first()
            .filter(|c| c.correlation.abs() >= LOOPBACK_MIN_CORRELATION)
            .map(|c| c.input);
        Ok(LoopbackDetection {
            backend: r.kind,
            input_device: r.input_device.clone(),
            output_device: r.output_device.clone(),
            output: r.output,
            level: Dbfs(r.level_dbfs),
            ranked,
            loopback,
            clock: conv::clock(n.clock),
        })
    })();
    handle.stop();
    let outcome = stream.stop(STOP_TIMEOUT);
    tracing::info!("loopback detection stream stopped: {outcome:?}");
    result
}
