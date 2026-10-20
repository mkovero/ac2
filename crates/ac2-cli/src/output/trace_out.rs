//! Trace tables: the stored traces list, one trace's metadata and its columns.

use super::*;

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
            let m = method.label();
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
            tr.kind.label().to_owned(),
            source_text(&tr.source),
            format::delay(tr.delay.0),
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
        TraceSource::Imported { notes, ir_file, .. } => ir_file
            .iter()
            .map(|f| format!("\n  file        {}", ac2_scene::trace::ir_file_text(f)))
            .chain(
                notes
                    .iter()
                    .map(|n| format!("\n  note        {}", ac2_scene::trace::import_note(*n))),
            )
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
        } => format!("\n  phase ref   {}", format::delay(delay.0)),
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
        t.kind.label(),
        source_text(&t.source),
        time_base(&t.source),
        format::delay(t.delay.0),
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
    let f = g.centres();
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
