//! Output: one JSON document, JSON lines, or human tables; numbers via `ac2-scene` format.

use std::io::{self, Write};

use ac2_proto::GridDef;
use ac2_proto::model::{
    Autosave, AutosaveState, Availability, BackendInfo, CalEntry, CalState, CalStatus,
    ClockRelation, DelayReference, DepthPolicy, MeasKind, Measurement, Mic, PeakMode, Polarity,
    RecordingEnd, RecordingFile, RecordingRun, Session, SessionFile, SmoothingFraction,
    SmoothingMode, State, TimingState, TimingStatus, TraceData, TraceMeta, TraceSource,
};
use ac2_proto::units::WallNs;
use ac2_scene::format;
use comfy_table::{Table, presets};
use serde::Serialize;

use ac2_scene::unit_text::channels_text;

mod trace_out;
pub use trace_out::{trace_columns, trace_meta, traces};

/// Output sink.
pub struct Out<'a> {
    /// `--json`.
    pub json: bool,
    /// Destination.
    pub w: &'a mut dyn Write,
    /// Reply warnings met on the way, in words; [`crate::run_reporting`] prints them on
    /// stderr once the command is done.
    pub warnings: Vec<String>,
}

impl std::fmt::Debug for Out<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Out").field("json", &self.json).finish()
    }
}

impl<'a> Out<'a> {
    /// A sink.
    pub fn new(json: bool, w: &'a mut dyn Write) -> Self {
        Self {
            json,
            w,
            warnings: Vec::new(),
        }
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

/// ` · avg FIFO 16 frames`, as the measurement list in the app says it; nothing when off.
fn spec_averaging(a: ac2_proto::model::SpecAveraging) -> String {
    match a {
        ac2_proto::model::SpecAveraging::Off => String::new(),
        a => format!(" · avg {}", ac2_scene::format::spec_averaging(a)),
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
        MeasKind::Spectrum { config } => format!(
            "spectrum in {} ({}{}){}",
            config.input + 1,
            ac2_scene::format::spec_resolution(config.resolution),
            ac2_scene::format::spec_level_shown(config.resolution, config.level)
                .map(|l| format!(" · {l}"))
                .unwrap_or_default(),
            spec_averaging(config.averaging)
        ),
        MeasKind::Rta { config } => format!(
            "rta 1/{} oct in {} ({}){}",
            config.fraction.b(),
            config.input + 1,
            config.weighting.letter(),
            spec_averaging(config.averaging)
        ),
        MeasKind::Spl { config } => format!(
            "spl in {} (L{}{})",
            config.input + 1,
            config.weighting.letter(),
            config.time_weighting.letter()
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
                ac2_proto::model::Operand::Eq { set, .. } => format!("EQ {}", set.0),
            });
            match &config.expr {
                ac2_proto::model::MathExpr::Average { method, .. } => {
                    format!("{what} math: {} {e}", method.label())
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
            format::dbfs(config.level.0),
            ac2_scene::unit_text::channels_text(&config.outputs)
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

/// Measurements table.
pub fn measurements(ms_: &[Measurement]) -> String {
    let mut t = table(&["id", "name", "kind", "running", "delay", "layout", "rev"]);
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
            m.layout.as_ref().map_or_else(
                || format::NO_VALUE.to_owned(),
                |l| format::own_panes(l.panes().len()),
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
    if let Some(l) = &m.layout {
        s.push_str(&format!("  {}", format::own_panes(l.panes().len())));
        // Panes numbered in reading order, as the app numbers them.
        for (i, p) in l.panes().iter().enumerate() {
            let mut said = Vec::new();
            if let Some(id) = p.meas.filter(|id| *id != m.id) {
                said.push(format!("shows {id}"));
            }
            if !p.levels.is_empty() {
                said.push("level axis set".to_owned());
            }
            if !said.is_empty() {
                s.push_str(&format!(" · pane {}: {}", i + 1, said.join(", ")));
            }
        }
    }
    if let Some(d) = m.config.kind.display() {
        s.push_str(&format!(
            "  {}{} · spectrograph {} s {}",
            d.style.name(),
            match d.peak {
                PeakMode::Off => String::new(),
                p => format!(", {}", p.name()),
            },
            d.spectrograph_span_s,
            d.spectrograph_colours.name()
        ));
    }
    if m.offset.0 != 0.0 {
        s.push_str(&format!("  offset {}", format::db_readout(m.offset.0)));
    }
    if !m.compared.is_empty() {
        s.push_str(&format!("  {} compared", m.compared.len()));
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
                |d| {
                    if d.system_default {
                        format!("{} (default)", d.max_channels)
                    } else {
                        d.max_channels.to_string()
                    }
                },
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
    for dev in backends.iter().flat_map(|b| &b.devices) {
        if let Some(clocks) = ac2_scene::audio::clock_sources_text(&dev.clock_sources) {
            out.push_str(&format!("\n{}: clock source {clocks}", dev.name));
        }
    }
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

/// The state files the daemon found damaged at start, a line each:
/// `damaged      rig settings rig.json damaged: …; set aside as rig.json.damaged-…`.
pub fn damaged_files(rig: &ac2_proto::model::Rig) -> Vec<String> {
    ac2_scene::autosave::damaged_file_lines(&rig.damaged_files)
        .into_iter()
        .map(|l| format!("damaged      {l}"))
        .collect()
}

/// The SPL logs the daemon cannot write, a line each:
/// `log          NOT SAVED: SPL log of Main not saved for 12 min: disk full`. Empty while
/// every log is on disk.
pub fn log_failures(a: &Autosave, measurements: &[Measurement], now: WallNs) -> Vec<String> {
    ac2_scene::autosave::log_failure_lines(
        &a.log_failures,
        |m| {
            measurements
                .iter()
                .find(|x| x.id == m)
                .map(|x| x.config.name.clone())
        },
        now,
        ac2_scene::time::ClockOffset(0),
    )
    .into_iter()
    .map(|l| format!("log          NOT SAVED: {l}"))
    .collect()
}

pub fn session(s: &Session) -> String {
    match &s.open {
        None => format!("audio I/O closed (epoch {})", s.epoch),
        Some(o) => match &o.replay {
            Some(r) => format!(
                "audio I/O open (epoch {}): replaying {} ({:?})\n  file   {}\n  input  {} ch {}\n  rate   {}  {} recorded",
                s.epoch,
                r.name,
                r.pace,
                r.path,
                o.input_device.0,
                channels_text(&o.config.input_channels),
                format::freq_readout(f64::from(o.sample_rate_hz)),
                ac2_scene::recording::clock(r.frames as f64 / f64::from(o.sample_rate_hz.max(1)))
            ),
            None => {
                format!(
                    "audio I/O open (epoch {})\n  input  {} ch {}\n  output {} × {}{}\n  rate   {}  buffer {} samples",
                    s.epoch,
                    o.input_device.0,
                    channels_text(&o.config.input_channels),
                    o.output_device.0,
                    o.config.output_channels,
                    if o.output_device != o.input_device && o.config.output_channels > 0 {
                        " (another device: the two clocks may drift)"
                    } else {
                        ""
                    },
                    format::freq_readout(f64::from(o.sample_rate_hz)),
                    o.buffer_frames
                ) + &driver_lines(o)
            }
        },
    }
}

/// A driver-reporting session's clock source and stated latency (ASIO, Core Audio), and a
/// change or reset the driver made under it.
fn driver_lines(o: &ac2_proto::model::OpenSession) -> String {
    let mut t = String::new();
    if let Some(c) = &o.clock_source {
        t.push_str(&format!("\n  clock  {c}"));
    }
    if let Some(l) = &o.stated_latency {
        t.push_str(&format!(
            "\n  {}",
            ac2_scene::audio::stated_latency_text(l, o.sample_rate_hz)
        ));
    }
    if let Some(c) = ac2_scene::audio::driver_reset_text(o) {
        t.push_str(&format!("\n  NOTE   {c}"));
    }
    if let Some(c) = ac2_scene::audio::driver_change_text(o) {
        t.push_str(&format!("\n  NOTE   {c}"));
    }
    t
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

/// `YYYY-MM-DD hh:mm` UTC of Unix nanoseconds.
pub fn utc(ns: u64) -> String {
    let secs = ns / 1_000_000_000;
    let (y, m, d) = ac2_proto::spl_log::civil_from_days(i64::try_from(secs / 86_400).unwrap_or(0));
    let rem = secs % 86_400;
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
            format::dbfs(e.spl.measured.0),
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
mod tests;
