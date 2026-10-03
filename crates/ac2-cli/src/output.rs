//! Output: one JSON document, JSON lines, or human tables; numbers via `ac2-scene` format.

use std::io::{self, Write};

use ac2_proto::GridDef;
use ac2_proto::model::{
    Availability, BackendInfo, CalEntry, CalState, CalStatus, DelayReference, DepthPolicy,
    InputSetup, LevelScale, MeasKind, Measurement, PeakWeighting, Polarity, Session, SessionFile,
    SmoothingFraction, SmoothingMode, TimeWeighting, TimingState, TimingStatus, TraceData,
    TraceKind, TraceMeta, TraceSource, Weighting,
};
use ac2_proto::units::WallNs;
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

/// Devices table: every device of every available backend, and a line per unavailable
/// backend saying why.
pub fn devices(backends: &[BackendInfo]) -> String {
    let mut t = table(&["backend", "id", "name", "in", "out", "rate", "clock"]);
    let mut unavailable = Vec::new();
    for b in backends {
        if let Availability::Unavailable { reason } = &b.availability {
            unavailable.push(format!(
                "{}: unavailable ({reason})",
                format!("{:?}", b.kind).to_lowercase()
            ));
        }
    }
    for dev in backends.iter().flat_map(|b| &b.devices) {
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
    let mut out = t.to_string();
    for u in unavailable {
        out.push('\n');
        out.push_str(&u);
    }
    out
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

fn source_text(s: &TraceSource) -> String {
    match s {
        TraceSource::Captured {
            meas, meas_name, ..
        } => format!("captured from {meas_name} ({meas})"),
        TraceSource::Imported { file_name, .. } => format!("imported {file_name}"),
        TraceSource::Average { traces, method, .. } => {
            let m = match method {
                ac2_proto::model::AverageMethod::Power => "power",
                ac2_proto::model::AverageMethod::Complex => "complex",
                ac2_proto::model::AverageMethod::CoherenceWeighted => "coherence-weighted",
            };
            format!(
                "{m} average of {}",
                traces
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        TraceSource::Math { a, b, op } => match op {
            ac2_proto::model::MathOp::MagnitudeDifference => format!("{a} − {b} (dB)"),
            ac2_proto::model::MathOp::ComplexDivision => format!("{a} / {b} (complex)"),
        },
        TraceSource::IrCapture { run, .. } => format!("sweep {run}"),
    }
}

fn kind_text(k: TraceKind) -> &'static str {
    match k {
        TraceKind::Transfer => "transfer",
        TraceKind::Target => "target",
        TraceKind::Spectrum { .. } => "spectrum",
        TraceKind::Rta { .. } => "rta",
        TraceKind::Sweep => "sweep",
    }
}

/// Traces table.
pub fn traces(t_: &[TraceMeta]) -> String {
    let mut t = table(&[
        "id",
        "slot",
        "name",
        "kind",
        "source",
        "delay",
        "time base",
        "shown",
    ]);
    for tr in t_ {
        t.add_row(vec![
            tr.id.to_string(),
            tr.edit.slot.map(|s| s.to_string()).unwrap_or_default(),
            tr.edit.name.clone(),
            kind_text(tr.kind).to_owned(),
            source_text(&tr.source),
            ms(tr.delay.0),
            time_base(&tr.source).to_owned(),
            yes(tr.edit.visible),
        ]);
    }
    t.to_string()
}

fn time_base(s: &TraceSource) -> &'static str {
    match s {
        TraceSource::Captured { .. } | TraceSource::IrCapture { .. } => "shared",
        _ => "indep.",
    }
}

/// Every metadata field of one trace.
pub fn trace_meta(t: &TraceMeta) -> String {
    let smoothing = match t.edit.smoothing {
        None => "none".to_owned(),
        Some(s) => {
            let f = match s.fraction {
                SmoothingFraction::Third => 3,
                SmoothingFraction::Sixth => 6,
                SmoothingFraction::Twelfth => 12,
                SmoothingFraction::TwentyFourth => 24,
                SmoothingFraction::FortyEighth => 48,
            };
            let m = match s.mode {
                SmoothingMode::Magnitude => "magnitude",
                SmoothingMode::MagnitudePhase => "magnitude and phase",
            };
            format!("1/{f} octave, {m}")
        }
    };
    let depth = match t.depth {
        None => "—".to_owned(),
        Some(DepthPolicy::EqualConfidence) => "equal confidence".to_owned(),
        Some(DepthPolicy::FastLf { max_settle_s }) => {
            format!("fast LF (≤ {} s)", format::fixed(max_settle_s.0, 1))
        }
    };
    let cal = match &t.cal {
        CalState::Uncalibrated => "uncalibrated".to_owned(),
        CalState::Calibrated {
            key, sensitivity, ..
        } => format!(
            "{} in {} mic {}, sensitivity {}",
            key.device.0,
            u32::from(key.channel) + 1,
            key.mic,
            format::db_readout(sensitivity.0)
        ),
    };
    let mic = t.mic.as_ref().map_or_else(
        || "—".to_owned(),
        |m| {
            format!(
                "{} (curve {})",
                m.name,
                m.curve.as_deref().unwrap_or("none")
            )
        },
    );
    let reference = match &t.source {
        TraceSource::Average {
            reference: DelayReference::Trace { trace },
            ..
        } => format!("\n  phase ref   delay of trace {trace}"),
        TraceSource::Average {
            reference: DelayReference::Fixed { delay },
            ..
        } => format!("\n  phase ref   {}", ms(delay.0)),
        _ => String::new(),
    };
    let epoch = match &t.source {
        TraceSource::Captured {
            epoch, at_sample, ..
        } => format!("\n  epoch       {} (sample {})", epoch.0, at_sample.0),
        TraceSource::IrCapture { epoch, .. } => format!("\n  epoch       {}", epoch.0),
        _ => String::new(),
    };
    format!(
        "trace {} {:?}{}\n  kind        {}\n  source      {}\n  time base   {}{epoch}\n  delay       {}{reference}\n  nudge       {}\n  polarity    {}\n  offset      {}\n  smoothing   {smoothing}\n  depth       {depth}\n  cal         {cal}\n  mic         {mic}\n  shown       {}{}\n  created     {} ns",
        t.id,
        t.edit.name,
        t.edit
            .slot
            .map_or_else(String::new, |s| format!(" (slot {s})")),
        kind_text(t.kind),
        source_text(&t.source),
        time_base(&t.source),
        ms(t.delay.0),
        ms(t.edit.delay_nudge.0),
        match t.edit.polarity {
            Polarity::Normal => "normal",
            Polarity::Inverted => "inverted",
        },
        format::db_readout(t.edit.offset.0),
        yes(t.edit.visible),
        if t.edit.locked { ", locked" } else { "" },
        t.created_at.0
    )
}

/// A trace's columns.
pub fn trace_columns(d: &TraceData, g: &GridDef) -> String {
    let f = ac2_scene::grid::column_frequencies(g);
    let mut head = vec!["freq", "mag"];
    if d.phase_deg.is_some() {
        head.push("phase");
    }
    if d.coherence.is_some() {
        head.push("γ²");
    }
    let mut t = table(&head);
    let num = |v: f32, dec: usize| {
        if v.is_finite() {
            format::fixed(f64::from(v), dec)
        } else {
            "—".to_owned()
        }
    };
    for (i, hz) in f.iter().enumerate() {
        let mut row = vec![
            format::freq_readout(*hz),
            num(d.mag_db.get(i).copied().unwrap_or(f32::NAN), 2),
        ];
        if let Some(p) = &d.phase_deg {
            row.push(num(p.get(i).copied().unwrap_or(f32::NAN), 1));
        }
        if let Some(c) = &d.coherence {
            row.push(num(c.get(i).copied().unwrap_or(f32::NAN), 3));
        }
        t.add_row(row);
    }
    t.to_string()
}

/// Saved sessions table.
pub fn sessions(l: &[SessionFile]) -> String {
    if l.is_empty() {
        return "no saved sessions".to_owned();
    }
    let mut t = table(&["name", "saved (UTC)", "meas", "traces", "path"]);
    for s in l {
        t.add_row(vec![
            s.name.clone(),
            utc(s.saved_at.0),
            s.measurements.to_string(),
            s.traces.to_string(),
            s.path.clone(),
        ]);
    }
    t.to_string()
}

/// `YYYY-MM-DD hh:mm` UTC of Unix nanoseconds (civil-from-days, proleptic Gregorian).
pub fn utc(ns: u64) -> String {
    let secs = ns / 1_000_000_000;
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
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
        "mic curve",
    ]);
    for e in c {
        let none = || format::NO_VALUE.to_owned();
        let (sens, calib, measured) = match &e.spl {
            Some(s) => (
                format::db_readout(s.sensitivity.0),
                format!(
                    "{} dB SPL @ {}",
                    format::level(s.calibrator_level.0),
                    format::freq_readout(s.calibrator_freq.0)
                ),
                dbfs(s.measured.0),
            ),
            None => (none(), none(), none()),
        };
        t.add_row(vec![
            e.key.device.0.clone(),
            (u32::from(e.key.channel) + 1).to_string(),
            e.key.mic.clone(),
            sens,
            calib,
            measured,
            e.mic_curve.as_ref().map_or_else(none, |c| {
                format!(
                    "{} ({} points, {} – {})",
                    c.file_name,
                    c.points,
                    format::freq_readout(c.f_lo.0),
                    format::freq_readout(c.f_hi.0)
                )
            }),
        ]);
    }
    t.to_string()
}

/// Input setup table (mic names, mic-curve switches).
pub fn inputs(i: &[InputSetup]) -> String {
    let mut t = table(&["in", "mic", "mic curve"]);
    for r in i {
        t.add_row(vec![
            (u32::from(r.channel) + 1).to_string(),
            r.mic.clone().unwrap_or_else(|| format::NO_VALUE.to_owned()),
            if r.mic_curve { "on" } else { "off" }.to_owned(),
        ]);
    }
    t.to_string()
}

/// Calibration state of a calibrated readout, in words (`ac2-scene` wording); `offset` is
/// the daemon − local clock offset, ns.
pub fn cal_status(cal: CalStatus, mic_curve: bool, now: WallNs, offset: i64) -> String {
    ac2_scene::spl::cal_text(cal, mic_curve, now, ac2_scene::time::ClockOffset(offset))
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
