//! ac2 CSV export: header lines for every metadata field, then the columns as measured.

use std::fmt::Write as _;

use ac2_proto::model::{
    CalState, DepthPolicy, DistortionCurve, LfHarmonics, MicState, Polarity, RoomAcoustics,
    RoomRefusal, RoomValue, SmoothingFraction, SmoothingMode, TraceIr, TraceKind, TraceMicCurve,
    TraceSource,
};

use crate::columns::StoredTrace;

use super::ac2::kind_header;
use super::{AC2_CSV_MAGIC, IR_HEADER, IrHeader, TransferIrHeader};

fn smoothing_text(s: Option<ac2_proto::model::Smoothing>) -> String {
    match s {
        None => "none".into(),
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
            format!("1/{f} octave, {m} (display only, not applied)")
        }
    }
}

fn source_text(s: &TraceSource) -> String {
    match s {
        TraceSource::Captured {
            meas,
            meas_name,
            epoch,
            at_sample,
        } => format!(
            "captured from {meas_name:?} (measurement {meas}), session epoch {}, sample {}",
            epoch.0, at_sample.0
        ),
        TraceSource::Imported {
            file_name,
            format,
            notes,
            ir_file,
        } => {
            let mut t = format!("imported from {file_name:?} ({format:?})");
            if !notes.is_empty() {
                t += &format!(", notes {notes:?}");
            }
            if let Some(f) = ir_file {
                t += &format!(
                    ", channel {} of {}, {} Hz, {} samples ({} kept from sample {}), peak at \
                     {:.4} ms, {:?}",
                    f.channel + 1,
                    f.channels,
                    f.sample_rate.0,
                    f.frames,
                    f.kept,
                    f.first,
                    f.peak.0 * 1e3,
                    f.import
                );
            }
            t
        }
        TraceSource::Generated { curve } => format!("generated: {curve:?}"),
        TraceSource::Eq { meas, set, rev } => {
            format!("filter response of EQ set {set} (rev {rev}) of measurement {meas}")
        }
        TraceSource::Average {
            traces,
            method,
            reference,
        } => format!(
            "{method:?} average of traces {}; phase reference {reference:?}",
            traces
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TraceSource::Math {
            meas,
            meas_name,
            epoch,
            at_sample,
            expr: _,
            operands,
            phase,
        } => format!(
            "captured from math channel {meas_name:?} (measurement {meas}) of {}, phase {phase:?}, session epoch {}, sample {}",
            operands
                .iter()
                .map(|m| format!("{:?}", m.name))
                .collect::<Vec<_>>()
                .join(", "),
            epoch.0,
            at_sample.0
        ),
        TraceSource::Sweep {
            meas,
            meas_name,
            run,
            number,
            epoch,
            sweep,
            level,
            repeats,
            lf_harmonics,
            reference_input,
            measurement_input,
        } => format!(
            "run {number} of sweep measurement {meas_name:?} (measurement {meas}, sweep {}; {} Hz – {} Hz, {} s, {} dBFS, {repeats}×{}), input {} re {}, session epoch {}",
            run.0,
            sweep.start.0,
            sweep.end.0,
            sweep.duration.0,
            level.0,
            match lf_harmonics {
                LfHarmonics::Standard => "",
                LfHarmonics::Fine => ", LF harmonics fine",
            },
            u32::from(*measurement_input) + 1,
            u32::from(*reference_input) + 1,
            epoch.0
        ),
    }
}

/// Shortest decimal that reads back as exactly `v`; `nan` for a gap.
fn num32(v: f32) -> String {
    if v.is_nan() {
        "nan".into()
    } else {
        format!("{v}")
    }
}

/// The ac2 CSV of a stored trace (module docs).
pub fn export_csv(t: &StoredTrace) -> String {
    let m = &t.meta;
    let mut s = String::new();
    let mut line = |k: &str, v: String| {
        let _ = writeln!(s, "# {k}: {}", v.replace(['\n', '\r'], " "));
    };
    line("name", m.edit.name.clone());
    line("kind", kind_header(m.kind).into());
    line("source", source_text(&m.source));
    line(
        "time_base",
        if m.source.shared_epoch().is_some() {
            "shared within its session epoch".into()
        } else {
            "independent".into()
        },
    );
    line("delay_ms", format!("{}", m.delay.0 * 1000.0));
    line(
        "delay_nudge_ms",
        format!("{}", m.edit.delay_nudge.0 * 1000.0),
    );
    line(
        "polarity",
        match m.edit.polarity {
            Polarity::Normal => "normal".into(),
            Polarity::Inverted => "inverted".into(),
        },
    );
    line("offset_db", format!("{}", m.edit.offset.0));
    line("smoothing", smoothing_text(m.edit.smoothing));
    line(
        "depth",
        match m.depth {
            None => "n/a".into(),
            Some(DepthPolicy::EqualConfidence) => "equal confidence".into(),
            Some(DepthPolicy::FastLf { max_settle_s }) => {
                format!("fast LF (max {} s)", max_settle_s.0)
            }
        },
    );
    line(
        "cal",
        match &m.cal {
            CalState::Uncalibrated => "uncalibrated".into(),
            CalState::Calibrated {
                key,
                sensitivity,
                calibrated_at,
            } => format!(
                "calibrated: device {:?} input {} mic {:?}, sensitivity {} dB, at {} ns",
                key.device.0,
                u32::from(key.channel) + 1,
                key.mic,
                sensitivity.0,
                calibrated_at.0
            ),
        },
    );
    line("mic", mic_text(m.mic.as_ref(), m.mic_curve.as_deref()));
    if let Some(mc) = &m.mic_curve {
        line(
            "mic_curve",
            serde_json::to_string(mc).unwrap_or_else(|_| "null".into()),
        );
    }
    line("created_ns", m.created_at.0.to_string());
    line(
        "note",
        "columns are as measured; offset, polarity, nudge, smoothing and a mic curve applied \
         after capture are display edits"
            .into(),
    );
    line(
        "grid",
        serde_json::to_string(&t.grid).unwrap_or_else(|_| "null".into()),
    );
    if let Some(sw) = &t.sweep {
        line(
            "sweep_info",
            serde_json::to_string(&sw.info).unwrap_or_else(|_| "null".into()),
        );
        line(
            "sweep_ir",
            serde_json::to_string(&IrHeader::of(&sw.ir)).unwrap_or_else(|_| "null".into()),
        );
        if let Some(u) = &sw.unresolved {
            line(
                "sweep_unresolved",
                serde_json::to_string(u).unwrap_or_else(|_| "null".into()),
            );
        }
        if let Some(r) = &sw.room {
            line(
                "room_metrics",
                serde_json::to_string(r).unwrap_or_else(|_| "null".into()),
            );
        }
    }
    let transfer_ir = t.ir.as_ref().filter(|_| m.kind == TraceKind::Transfer);
    if let Some(ti) = transfer_ir {
        let h = IrHeader::of(&ti.ir);
        let h = TransferIrHeader {
            sample_rate: ti.sample_rate,
            t0: h.t0,
            dt: h.dt,
            points: h.points,
        };
        line(
            "transfer_ir",
            serde_json::to_string(&h).unwrap_or_else(|_| "null".into()),
        );
    }
    let mut out = format!("{AC2_CSV_MAGIC}\n{s}");
    let c = &t.columns;
    out.push_str("freq_hz,mag_db");
    if c.phase_deg.is_some() {
        out.push_str(",phase_deg");
    }
    if c.coherence.is_some() {
        out.push_str(",coherence");
    }
    let curves: Vec<&DistortionCurve> = t.sweep.as_ref().map_or_else(Vec::new, |s| {
        s.harmonics
            .iter()
            .map(|h| &h.curve)
            .chain(std::iter::once(&s.thd))
            .chain(s.reference_harmonics.iter().flatten().map(|h| &h.curve))
            .collect()
    });
    if let Some(s) = &t.sweep {
        for h in &s.harmonics {
            let _ = write!(out, ",h{0}_db,h{0}_floor_db", h.order);
        }
        out.push_str(",thd_db,thd_floor_db");
        for h in s.reference_harmonics.iter().flatten() {
            let _ = write!(out, ",ref_h{0}_db,ref_h{0}_floor_db", h.order);
        }
    }
    out.push('\n');
    for (i, f) in t.grid.centres().iter().enumerate() {
        let _ = write!(
            out,
            "{f},{}",
            num32(c.mag_db.get(i).copied().unwrap_or(f32::NAN))
        );
        if let Some(p) = &c.phase_deg {
            let _ = write!(out, ",{}", num32(p.get(i).copied().unwrap_or(f32::NAN)));
        }
        if let Some(k) = &c.coherence {
            let _ = write!(out, ",{}", num32(k.get(i).copied().unwrap_or(f32::NAN)));
        }
        for d in &curves {
            let _ = write!(
                out,
                ",{},{}",
                num32(d.level_db.get(i).copied().unwrap_or(f32::NAN)),
                num32(d.floor_db.get(i).copied().unwrap_or(f32::NAN))
            );
        }
        out.push('\n');
    }
    if let Some(sw) = &t.sweep {
        out.push_str(
            "# impulse response: time re the arrival (s), signed extreme re the reference, \
             envelope (dB)\n",
        );
        ir_rows(&mut out, &sw.ir);
        if let Some(r) = &sw.room {
            room_table(&mut out, r);
        }
    }
    if let Some(ti) = transfer_ir {
        out.push_str(
            "# impulse response: time re the inserted delay (s), linear re the reference, \
             envelope (dB)\n",
        );
        ir_rows(&mut out, &ti.ir);
    }
    out
}

/// An impulse-response table: its column header, then one row per point.
fn ir_rows(out: &mut String, ir: &TraceIr) {
    out.push_str(IR_HEADER);
    out.push('\n');
    for (i, (l, e)) in ir.linear.iter().zip(&ir.etc_db).enumerate() {
        let at = ir.t0.0 + i as f64 * ir.dt.0;
        let _ = writeln!(out, "{at},{},{}", num32(*l), num32(*e));
    }
}

/// The room parameters as comment lines, for reading (the import uses `room_metrics`).
fn room_table(out: &mut String, r: &RoomAcoustics) {
    let value = |v: RoomValue| match v {
        RoomValue::Value { value } => format!("{value}"),
        RoomValue::Refused { reason } => match reason {
            RoomRefusal::NoDecay => "refused:no_decay".into(),
            RoomRefusal::InsufficientRange { range, needed } => {
                format!("refused:decay_range_{}_of_{}_db", range.0, needed.0)
            }
            RoomRefusal::FilterLimited { .. } => "refused:decay_too_short_for_band".into(),
        },
    };
    out.push_str(
        "# room parameters (ISO 3382-1), decay times in s, C50/C80 in dB, D50 0..1, times \
         re the arrival:\n",
    );
    out.push_str(
        "# band_hz,edt_s,t20_s,t30_s,c50_db,c80_db,d50,decay_range_db,curvature_pct,onset_s,\
         truncation_s\n",
    );
    for (set, bands) in [
        ("broadband", std::slice::from_ref(&r.broadband)),
        ("octave", r.octave.as_slice()),
        ("third", r.third.as_slice()),
    ] {
        for b in bands {
            let band = b
                .centre
                .map_or_else(|| set.to_owned(), |c| format!("{}", c.0));
            let curvature = b.curvature.map_or_else(|| "nan".into(), |c| format!("{c}"));
            let _ = writeln!(
                out,
                "# {band},{},{},{},{},{},{},{},{curvature},{},{}",
                value(b.edt),
                value(b.t20),
                value(b.t30),
                value(b.c50),
                value(b.c80),
                value(b.d50),
                b.decay_range.map_or(f64::NAN, |r| r.0),
                b.onset.0,
                b.truncation.0
            );
        }
    }
}

/// The `# mic:` header: the mic at capture with the curve its columns carry, or the curve
/// applied afterwards as a display edit.
fn mic_text(mic: Option<&MicState>, applied: Option<&TraceMicCurve>) -> String {
    match (mic, applied) {
        (_, Some(a)) => {
            let captured = match mic {
                Some(m) if m.name != a.mic => format!("; captured with mic {}", m.name),
                _ => String::new(),
            };
            format!(
                "{} (curve: {}, applied after capture as a display edit, not in the columns; \
                 0 dB at {} Hz; file {:?}, hash {}{captured})",
                a.mic, a.curve.label, a.f_norm.0, a.curve.file_name, a.curve.content_hash
            )
        }
        (None, None) => "none".into(),
        (Some(m), None) => match &m.curve {
            Some(c) => format!(
                "{} (curve: {}, in the columns; file {:?}, hash {})",
                m.name, c.label, c.file_name, c.content_hash
            ),
            None => format!("{} (curve: none)", m.name),
        },
    }
}
