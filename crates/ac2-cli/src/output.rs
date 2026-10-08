//! Output: one JSON document, JSON lines, or human tables; numbers via `ac2-scene` format.

use std::io::{self, Write};

use ac2_proto::GridDef;
use ac2_proto::model::{
    Autosave, AutosaveState, Availability, BackendInfo, CalEntry, CalState, CalStatus,
    ClockRelation, DelayReference, DepthPolicy, LevelScale, MeasKind, Measurement, Mic,
    PeakWeighting, Polarity, RecordingEnd, RecordingFile, RecordingRun, Session, SessionFile,
    SmoothingFraction, SmoothingMode, State, TimeWeighting, TimingState, TimingStatus, TraceData,
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
        MeasKind::Math { config } => {
            let what = match config.domain {
                ac2_proto::model::MathDomain::Transfer => "tf",
                ac2_proto::model::MathDomain::Spectrum => "spectrum",
                ac2_proto::model::MathDomain::Rta => "rta",
            };
            let e = ac2_scene::math::expression(&config.expr, |o| match o {
                ac2_proto::model::Operand::Meas { meas } => format!("#{meas}"),
                ac2_proto::model::Operand::Trace { trace } => format!("trace {trace}"),
            });
            match &config.expr {
                ac2_proto::model::MathExpr::Average { method, .. } => {
                    format!("{what} math: {} {e}", average_method(*method))
                }
                ac2_proto::model::MathExpr::Binary { .. } => format!("{what} math: {e}"),
            }
        }
        MeasKind::Sweep { config } => format!(
            "sweep in {} re in {}, {} – {}, {} s × {} at {} on out {}",
            config.measurement_input + 1,
            config.reference_input + 1,
            format::freq_readout(config.sweep.start.0),
            format::freq_readout(config.sweep.end.0),
            format::fixed(config.sweep.duration.0, 1),
            config.repeats,
            dbfs(config.level.0),
            crate::units::channels_text(&config.outputs)
        ),
    }
}

/// Where a trace or math channel is filed: `#3` (measurement 3) or `imported`.
pub fn owner_text(o: ac2_proto::model::TraceOwner) -> String {
    match o {
        ac2_proto::model::TraceOwner::Meas { meas } => format!("#{meas}"),
        ac2_proto::model::TraceOwner::Imported => "imported".into(),
    }
}

/// `power`, `complex`, `coherence-weighted`.
pub fn average_method(m: ac2_proto::model::AverageMethod) -> &'static str {
    match m {
        ac2_proto::model::AverageMethod::Power => "power",
        ac2_proto::model::AverageMethod::Complex => "complex",
        ac2_proto::model::AverageMethod::CoherenceWeighted => "coherence-weighted",
    }
}

/// Measurements table.
pub fn measurements(ms_: &[Measurement]) -> String {
    let mut t = table(&["id", "name", "kind", "running", "delay", "rev"]);
    for m in ms_ {
        t.add_row(vec![
            m.id.to_string(),
            m.config.name.clone(),
            meas_kind(&m.config.kind),
            yes(m.running),
            m.delay.as_ref().map_or_else(
                || format::NO_VALUE.to_owned(),
                |d| format::delay_and_offset(d.applied.0, d.nudged.0),
            ),
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
        // One delay and its offset from the measured arrival, as the app's row says it.
        s.push_str(&format!(
            "  {} · {} samples{}",
            format::meas_delay(d.applied.0, d.nudged.0),
            format::fixed(d.applied_samples, 2),
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
/// The daemon's autosave as one line: `autosave     autosaved 5 min ago`, `saving…`,
/// `failed: <reason>` (in full), `off`. The age assumes this machine's clock matches the
/// daemon's.
pub fn autosave(a: &Autosave, now: WallNs) -> String {
    let text = match &a.state {
        AutosaveState::Failed { reason } => format!("FAILED: {reason}"),
        _ => ac2_scene::autosave::autosave_label(a, now, ac2_scene::time::ClockOffset(0))
            .map_or_else(|| "off".to_owned(), |l| l.text),
    };
    format!("autosave     {text}")
}

pub fn session(s: &Session) -> String {
    match &s.open {
        None => format!("session closed (epoch {})", s.epoch),
        Some(o) => match &o.replay {
            Some(r) => format!(
                "session open (epoch {}): replaying {} ({:?})\n  file   {}\n  input  {} ch {}\n  rate   {}  {} recorded",
                s.epoch,
                r.name,
                r.pace,
                r.path,
                o.input_device.0,
                channels_text(&o.config.input_channels),
                format::freq_readout(f64::from(o.sample_rate_hz)),
                ac2_scene::recording::clock(r.frames as f64 / f64::from(o.sample_rate_hz.max(1)))
            ),
            None => format!(
                "session open (epoch {})\n  input  {} ch {}\n  output {} × {}\n  rate   {}  buffer {} samples",
                s.epoch,
                o.input_device.0,
                channels_text(&o.config.input_channels),
                o.output_device.0,
                o.config.output_channels,
                format::freq_readout(f64::from(o.sample_rate_hz)),
                o.buffer_frames
            ),
        },
    }
}

fn input_label(i: u16) -> String {
    format!("in {}", i + 1)
}

/// One line for a recording: the indicator's text.
pub fn recording_line(r: &RecordingRun) -> String {
    ac2_scene::recording::recording_label(r, input_label).text
}

/// A recording: the indicator and its detail.
pub fn recording(r: &RecordingRun) -> String {
    let l = ac2_scene::recording::recording_label(r, input_label);
    format!("{}\n  {}", l.text, l.detail)
}

/// `rec list`.
pub fn recordings(l: &[RecordingFile]) -> String {
    if l.is_empty() {
        return "no recordings".to_owned();
    }
    let mut t = table(&[
        "name",
        "started (UTC)",
        "inputs",
        "length",
        "dropouts",
        "ended",
        "path",
    ]);
    for r in l {
        let secs = r.frames as f64 / f64::from(r.sample_rate_hz.max(1));
        t.add_row(vec![
            r.name.clone(),
            utc(r.started_at.0),
            channels_text(&r.inputs),
            ac2_scene::recording::clock(secs),
            r.discontinuities.to_string(),
            r.end.as_ref().map_or_else(
                || "recording".to_owned(),
                |e| match e {
                    RecordingEnd::WriteFailed { msg } => format!("write failed: {msg}"),
                    other => format!("{other:?}"),
                },
            ),
            r.path.clone(),
        ]);
    }
    t.to_string()
}

fn source_text(s: &TraceSource) -> String {
    match s {
        TraceSource::Captured {
            meas, meas_name, ..
        } => format!("captured from {meas_name} ({meas})"),
        TraceSource::Imported { file_name, .. } => format!("imported {file_name}"),
        TraceSource::Math {
            meas_name,
            expr,
            operands,
            ..
        } => format!(
            "math {meas_name}: {}",
            ac2_scene::math::expression(expr, |o| operands
                .iter()
                .find(|n| n.operand == o)
                .map_or_else(|| "?".to_owned(), |n| n.name.clone()))
        ),
        TraceSource::Average { traces, method, .. } => {
            let m = average_method(*method);
            format!(
                "{m} average of {}",
                traces
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        TraceSource::Sweep {
            meas_name, number, ..
        } => format!("run {number} of {meas_name}"),
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
        "under",
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
            owner_text(tr.edit.owner),
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
    if s.shared_epoch().is_some() {
        "shared"
    } else {
        "indep."
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
    let mic = ac2_scene::trace::mic_text(t.mic.as_ref(), t.mic_curve.as_deref());
    let notes: String = match &t.source {
        TraceSource::Imported { notes, .. } => notes
            .iter()
            .map(|n| format!("\n  note        {}", ac2_scene::trace::import_note(*n)))
            .collect(),
        _ => String::new(),
    };
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
        }
        | TraceSource::Math {
            epoch, at_sample, ..
        } => format!("\n  epoch       {} (sample {})", epoch.0, at_sample.0),
        TraceSource::Sweep { epoch, .. } => format!("\n  epoch       {}", epoch.0),
        _ => String::new(),
    };
    format!(
        "trace {} {:?}{}\n  under       {}\n  kind        {}\n  source      {}\n  time base   {}{epoch}\n  delay       {}{reference}\n  from arrival {}\n  polarity    {}\n  offset      {}\n  smoothing   {smoothing}\n  depth       {depth}\n  cal         {cal}\n  mic         {mic}\n  shown       {}{}\n  created     {} ns{notes}",
        t.id,
        t.edit.name,
        t.edit
            .slot
            .map_or_else(String::new, |s| format!(" (slot {s})")),
        owner_text(t.edit.owner),
        kind_text(t.kind),
        source_text(&t.source),
        time_base(&t.source),
        ms(t.delay.0),
        format::arrival_offset(t.edit.delay_nudge.0),
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

/// The local UTC offset (s) in force at wall time `t`.
pub fn local_offset_s(t: ac2_proto::units::WallNs) -> i32 {
    use chrono::{Local, Offset, TimeZone};
    let ns = i64::try_from(t.0).unwrap_or(i64::MAX);
    Local.timestamp_nanos(ns).offset().fix().local_minus_utc()
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

/// Sensitivity calibrations table: what 0 dBFS is, how it was found and how sure.
pub fn calibrations(c: &[CalEntry]) -> String {
    let mut t = table(&[
        "device",
        "in",
        "mic",
        "sensitivity",
        "method",
        "uncertainty",
        "measured",
        "calibrated (UTC)",
    ]);
    for e in c {
        t.add_row(vec![
            e.key.device.0.clone(),
            (u32::from(e.key.channel) + 1).to_string(),
            e.key.mic.clone(),
            format::db_readout(e.spl.sensitivity.0),
            ac2_scene::cal::method_cell(e),
            ac2_scene::cal::uncertainty_cell(e),
            dbfs(e.spl.measured.0),
            utc(e.spl.calibrated_at.0),
        ]);
    }
    t.to_string()
}

/// An electrical calibration just taken: the table row, then the numbers it rests on, the
/// notes and what to do with the phantom power now.
pub fn electrical_calibration(e: &CalEntry, c: ac2_proto::model::ElectricalConnection) -> String {
    let mut s = calibrations(std::slice::from_ref(e));
    s.push('\n');
    s.push_str(&ac2_scene::cal::method_detail(e));
    s.push_str(&format!(
        "\n0 dBFS = {} dB SPL on input {} ({})",
        format::level(e.spl.sensitivity.0),
        u32::from(e.key.channel) + 1,
        e.key.mic
    ));
    for n in ac2_scene::cal::electrical_notes(e) {
        s.push_str("\nnote: ");
        s.push_str(&n);
    }
    s.push('\n');
    s.push_str(ac2_scene::cal::electrical_after(c));
    s
}

/// The mic library: one row per curve.
pub fn mics(m: &[Mic]) -> String {
    let mut t = table(&["mic", "curve", "file", "points", "range", "data sheet"]);
    for mic in m {
        for c in &mic.curves {
            t.add_row(vec![
                mic.name.clone(),
                c.label.clone(),
                c.file_name.clone(),
                c.points.to_string(),
                format!(
                    "{} – {}",
                    format::freq_readout(c.f_lo.0),
                    format::freq_readout(c.f_hi.0)
                ),
                c.stated_sensitivity.map_or_else(
                    || format::NO_VALUE.to_owned(),
                    ac2_scene::cal::stated_sensitivity,
                ),
            ]);
        }
    }
    t.to_string()
}

/// The channels the input table lists: every input with a setup row, and every input the
/// open session captures.
fn input_channels(s: &State) -> Vec<u16> {
    let mut ch: Vec<u16> = s.inputs.iter().map(|i| i.channel).collect();
    if let Some(o) = &s.session.open {
        ch.extend(o.config.input_channels.iter().copied());
    }
    ch.sort_unstable();
    ch.dedup();
    ch
}

/// What each input uses: its mic, its mic curve (or why none applies) and its sensitivity
/// calibration, in the app's words (`ac2_scene::cal`). The age is on this machine's clock.
pub fn inputs(s: &State, now: WallNs) -> String {
    let mut t = table(&["in", "mic", "mic curve", "sensitivity"]);
    for ch in input_channels(s) {
        let u = ac2_proto::cal::state_input_use(s, ch);
        t.add_row(vec![
            (u32::from(ch) + 1).to_string(),
            u.mic.unwrap_or(format::NO_VALUE).to_owned(),
            ac2_scene::cal::curve_state(&u.curve),
            ac2_scene::cal::sensitivity_state(&u.sensitivity, now, ac2_scene::time::ClockOffset(0)),
        ]);
    }
    t.to_string()
}

/// [`inputs`] as JSON rows.
pub fn inputs_json(s: &State) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = input_channels(s)
        .into_iter()
        .map(|ch| {
            let row = ac2_proto::cal::input_setup(&s.inputs, ch);
            let u = ac2_proto::cal::state_input_use(s, ch);
            serde_json::json!({
                "input": u32::from(ch) + 1,
                "mic": row.mic,
                "curve": row.curve,
                "curve_text": ac2_scene::cal::curve_state(&u.curve),
                "curve_applied": u.curve.applied(),
                "sensitivity": u.sensitivity.entry(),
                "cal": u.sensitivity.status(),
            })
        })
        .collect();
    serde_json::Value::Array(rows)
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

/// The session's clock domain, as one line: what the backend states about input and output
/// clocks and, once the loopback monitor has measured it, their drift
/// (`docs/design/multi-device.md`). `None` while no session is open.
pub fn clock(session: &Session, t: &TimingStatus) -> Option<String> {
    let open = session.open.as_ref()?;
    let relation = match open.clock {
        ClockRelation::SingleCallback => "one clock (one callback for input and output)",
        ClockRelation::SameDeviceSeparateCallbacks => {
            "one device (separate input and output callbacks)"
        }
        ClockRelation::Unknown => "input and output may be on different clocks",
    };
    let drift = match &t.drift {
        None if open.config.loopback.is_none() => "drift not measured (no loopback)".to_owned(),
        None => "drift not measured yet (needs a stimulus)".to_owned(),
        Some(d) => format!(
            "drift {} ppm over {}{}",
            format::signed(d.ppm, 1),
            format::duration(d.span.0),
            if d.warning {
                "  WARNING: output and input on different clocks"
            } else {
                ""
            }
        ),
    };
    Some(format!("clock        {relation}; {drift}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autosave_line() {
        const S: u64 = 1_000_000_000;
        let now = WallNs(1_000 * S);
        let a = |state, saved_at: Option<u64>| Autosave {
            state,
            saved_at: saved_at.map(WallNs),
        };
        assert_eq!(
            autosave(&a(AutosaveState::Off, None), now),
            "autosave     off"
        );
        assert_eq!(
            autosave(&a(AutosaveState::Saved, Some(400 * S)), now),
            "autosave     autosaved 10 min ago"
        );
        assert_eq!(
            autosave(&a(AutosaveState::Pending, None), now),
            "autosave     saving…"
        );
        let reason = "x".repeat(100);
        assert_eq!(
            autosave(
                &a(
                    AutosaveState::Failed {
                        reason: reason.clone()
                    },
                    None
                ),
                now
            ),
            format!("autosave     FAILED: {reason}")
        );
    }
}
