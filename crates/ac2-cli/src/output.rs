//! Output: one JSON document, JSON lines, or human tables; numbers via `ac2-scene` format.

use std::io::{self, Write};

use ac2_proto::model::{
    CalEntry, DeviceInfo, LevelScale, MeasKind, Measurement, PeakWeighting, Session, TimeWeighting,
    TimingState, TimingStatus, TraceMeta, TraceSource, Weighting,
};
use ac2_scene::format;
use comfy_table::{Table, presets};
use serde::Serialize;

use crate::units::channels_text;

/// Output sink.
pub struct Out<'a> {
    /// `--json`.
    pub json: bool,
    /// Destination.
    pub w: &'a mut dyn Write,
}

impl std::fmt::Debug for Out<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Out").field("json", &self.json).finish()
    }
}

impl<'a> Out<'a> {
    /// A sink.
    pub fn new(json: bool, w: &'a mut dyn Write) -> Self {
        Self { json, w }
    }

    /// Prints `value` as pretty JSON, or the human text.
    pub fn emit<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
        human: impl FnOnce() -> String,
    ) -> io::Result<()> {
        if self.json {
            let s = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
            writeln!(self.w, "{s}")
        } else {
            let h = human();
            if h.ends_with('\n') {
                write!(self.w, "{h}")
            } else {
                writeln!(self.w, "{h}")
            }
        }
    }

    /// One JSON line (live views).
    pub fn json_line<T: Serialize + ?Sized>(&mut self, value: &T) -> io::Result<()> {
        let s = serde_json::to_string(value).map_err(io::Error::other)?;
        writeln!(self.w, "{s}")?;
        self.w.flush()
    }
}

/// A table with the CLI's style.
pub fn table(header: &[&str]) -> Table {
    let mut t = Table::new();
    t.load_preset(presets::UTF8_HORIZONTAL_ONLY);
    t.set_header(header.iter().copied());
    t
}

/// `12.50 ms`.
pub fn ms(seconds: f64) -> String {
    format::ms(seconds, 2)
}

/// `−20.0 dBFS`.
pub fn dbfs(v: f64) -> String {
    let s = format::level(v);
    if s == format::NO_VALUE {
        s
    } else {
        format!("{s} dBFS")
    }
}

/// Level with its scale's unit.
pub fn level(v: f64, scale: LevelScale) -> String {
    let s = format::level(v);
    if s == format::NO_VALUE {
        return s;
    }
    match scale {
        LevelScale::Dbfs => format!("{s} dBFS"),
        LevelScale::DbSpl => format!("{s} dB SPL"),
    }
}

/// `A`, `C`, `Z`.
pub fn weighting(w: Weighting) -> &'static str {
    match w {
        Weighting::A => "A",
        Weighting::C => "C",
        Weighting::Z => "Z",
    }
}

/// `F`, `S`, `I`.
pub fn time_weighting(t: TimeWeighting) -> &'static str {
    match t {
        TimeWeighting::Fast => "F",
        TimeWeighting::Slow => "S",
        TimeWeighting::Impulse => "I",
    }
}

/// `C`, `Z`.
pub fn peak_weighting(p: PeakWeighting) -> &'static str {
    match p {
        PeakWeighting::C => "C",
        PeakWeighting::Z => "Z",
    }
}

/// One-line description of what a measurement computes (channels 1-based).
pub fn meas_kind(k: &MeasKind) -> String {
    match k {
        MeasKind::Transfer { config } => format!(
            "tf ref {} → meas {}",
            config.reference_input + 1,
            config.measurement_input + 1
        ),
        MeasKind::Spectrum { config } => {
            format!("spectrum in {} ({} pt)", config.input + 1, config.fft_len)
        }
        MeasKind::Rta { config } => format!(
            "rta 1/{} oct in {} ({})",
            config.fraction.b(),
            config.input + 1,
            weighting(config.weighting)
        ),
        MeasKind::Spl { config } => format!(
            "spl in {} (L{}{})",
            config.input + 1,
            weighting(config.weighting),
            time_weighting(config.time_weighting)
        ),
    }
}

/// Measurements table.
pub fn measurements(ms_: &[Measurement]) -> String {
    let mut t = table(&["id", "name", "kind", "running", "frozen", "delay", "rev"]);
    for m in ms_ {
        t.add_row(vec![
            m.id.to_string(),
            m.config.name.clone(),
            meas_kind(&m.config.kind),
            yes(m.running),
            yes(m.frozen),
            m.delay
                .as_ref()
                .map_or_else(|| format::NO_VALUE.to_owned(), |d| ms(d.applied.0)),
            m.config_rev.to_string(),
        ]);
    }
    t.to_string()
}

/// One measurement, as a line.
pub fn measurement(m: &Measurement) -> String {
    let mut s = format!(
        "{} {}  {}  {}",
        m.id,
        m.config.name,
        meas_kind(&m.config.kind),
        if m.running { "running" } else { "stopped" }
    );
    if let Some(d) = &m.delay {
        s.push_str(&format!(
            "  delay {} ({} samples){}",
            ms(d.applied.0),
            d.applied_samples.0,
            if d.tracking { ", tracking" } else { "" }
        ));
    }
    s
}

/// `yes` / `no`.
pub fn yes(b: bool) -> String {
    if b { "yes" } else { "no" }.to_owned()
}

/// Devices table.
pub fn devices(d: &[DeviceInfo]) -> String {
    let mut t = table(&["backend", "id", "name", "in", "out", "rate", "clock"]);
    for dev in d {
        let ch = |x: &Option<ac2_proto::model::DirectionInfo>| {
            x.as_ref().map_or_else(
                || format::NO_VALUE.to_owned(),
                |d| d.max_channels.to_string(),
            )
        };
        let rate = dev
            .input
            .as_ref()
            .or(dev.output.as_ref())
            .and_then(|d| d.default_rate_hz)
            .map_or_else(
                || format::NO_VALUE.to_owned(),
                |r| format::freq_readout(f64::from(r)),
            );
        t.add_row(vec![
            format!("{:?}", dev.backend).to_lowercase(),
            dev.id.0.clone(),
            dev.name.clone(),
            ch(&dev.input),
            ch(&dev.output),
            rate,
            format!("{:?}", dev.duplex_clock),
        ]);
    }
    t.to_string()
}

/// Session, as lines.
pub fn session(s: &Session) -> String {
    match &s.open {
        None => format!("session closed (epoch {})", s.epoch),
        Some(o) => format!(
            "session open (epoch {})\n  input  {} ch {}\n  output {} × {}\n  rate   {}  buffer {} samples",
            s.epoch,
            o.input_device.0,
            channels_text(&o.config.input_channels),
            o.output_device.0,
            o.config.output_channels,
            format::freq_readout(f64::from(o.sample_rate_hz)),
            o.buffer_frames
        ),
    }
}

/// Traces table.
pub fn traces(t_: &[TraceMeta]) -> String {
    let mut t = table(&["id", "name", "source", "delay", "visible"]);
    for tr in t_ {
        let src = match &tr.source {
            TraceSource::Captured { meas, .. } => format!("captured from {meas}"),
            TraceSource::Imported { file_name, .. } => format!("imported {file_name}"),
            TraceSource::Average { traces, .. } => format!("average of {}", traces.len()),
            TraceSource::Math { a, b, .. } => format!("{a} − {b}"),
            TraceSource::IrCapture { .. } => "ir capture".to_owned(),
        };
        t.add_row(vec![
            tr.id.to_string(),
            tr.edit.name.clone(),
            src,
            ms(tr.delay.0),
            yes(tr.edit.visible),
        ]);
    }
    t.to_string()
}

/// Calibrations table.
pub fn calibrations(c: &[CalEntry]) -> String {
    let mut t = table(&[
        "device",
        "in",
        "mic",
        "sensitivity",
        "calibrator",
        "measured",
    ]);
    for e in c {
        t.add_row(vec![
            e.key.device.0.clone(),
            (u32::from(e.key.channel) + 1).to_string(),
            e.key.mic.clone(),
            format::db_readout(e.sensitivity.0),
            format!(
                "{} dB SPL @ {}",
                format::level(e.calibrator_level.0),
                format::freq_readout(e.calibrator_freq.0)
            ),
            dbfs(e.measured.0),
        ]);
    }
    t.to_string()
}

/// Timing state, in words.
pub fn timing_state(s: &TimingState, rate: Option<u32>) -> String {
    let samples = |n: i64| match rate {
        Some(r) if r > 0 => format!("{n} samples ({})", ms(n as f64 / f64::from(r))),
        _ => format!("{n} samples"),
    };
    match s {
        TimingState::NoStimulus => "no stimulus".to_owned(),
        TimingState::Acquiring => "acquiring".to_owned(),
        TimingState::Locked { offset } => format!("locked, offset {}", samples(offset.0)),
        TimingState::Jumped { from, to } => {
            format!("jumped {} → {}", samples(from.0), samples(to.0))
        }
        TimingState::Lost => "LOST".to_owned(),
    }
}

/// Timing status, as lines.
pub fn timing(t: &TimingStatus, rate: Option<u32>) -> String {
    let mut s = format!("timing   {}", timing_state(&t.state, rate));
    if let Some(d) = &t.drift {
        s.push_str(&format!(
            "\ndrift    {} ppm over {}{}",
            format::signed(d.ppm, 2),
            format::duration(d.span.0),
            if d.warning { "  WARNING" } else { "" }
        ));
    }
    s.push_str(&format!(
        "\nreference {}",
        if t.internal_reference {
            "internal loopback"
        } else {
            "none"
        }
    ));
    s
}
