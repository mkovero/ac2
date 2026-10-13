//! ac2 CSV import: the header lines, the data table and the impulse-response table.

use ac2_proto::GridDef;
use ac2_proto::ImportProblem;
use ac2_proto::model::{
    DistortionCurve, HarmonicCurve, ImportFormat, ImportNote, RoomAcoustics, SweepData, SweepInfo,
    SweepUnresolved, TraceIr, TraceKind, TransferIr,
};
use ac2_proto::units::Seconds;

use super::table::{
    Delim, Layout, is_comment, number, number32, on_import_grid, positional, split, table, validate,
};
use super::{
    AC2_CSV_MAGIC, AC2_CSV_MAGIC_V1, AC2_CSV_MAGIC_V2, AC2_CSV_MAGIC_V3, AC2_CSV_MAGIC_V4,
    AC2_CSV_MAGIC_V5, AC2_CSV_PREFIX, IR_HEADER, ImportError, Imported, IrAnnounce, fail,
};

pub(super) fn kind_from_header(v: &str) -> Option<TraceKind> {
    use ac2_proto::model::{LevelScale, SpecLevel};
    let spectrum = |scale, level| TraceKind::Spectrum { scale, level };
    Some(match v.trim() {
        "transfer" => TraceKind::Transfer,
        "target" => TraceKind::Target,
        "spectrum dBFS" => spectrum(LevelScale::Dbfs, SpecLevel::Tone),
        "spectrum dB SPL" => spectrum(LevelScale::DbSpl, SpecLevel::Tone),
        "spectrum dBFS/Hz" => spectrum(LevelScale::Dbfs, SpecLevel::Density),
        "spectrum dB SPL/Hz" => spectrum(LevelScale::DbSpl, SpecLevel::Density),
        "rta dBFS" => TraceKind::Rta {
            scale: LevelScale::Dbfs,
        },
        "rta dB SPL" => TraceKind::Rta {
            scale: LevelScale::DbSpl,
        },
        "sweep" => TraceKind::Sweep,
        _ => return None,
    })
}

/// The kind an ac2 CSV's header names (`# kind: target`), if it names one it reads: how a
/// salvage tells a target file from a measured one before importing it.
pub fn header_kind(content: &[u8]) -> Option<TraceKind> {
    let text = std::str::from_utf8(content).ok()?;
    text.lines()
        .map(str::trim)
        .take_while(|l| l.is_empty() || l.starts_with('#'))
        .find_map(|l| l.strip_prefix("# kind: "))
        .and_then(kind_from_header)
}

pub(super) fn kind_header(k: TraceKind) -> &'static str {
    use ac2_proto::model::{LevelScale, SpecLevel};
    match k {
        TraceKind::Transfer => "transfer",
        TraceKind::Target => "target",
        TraceKind::Spectrum { scale, level } => match (scale, level) {
            (LevelScale::Dbfs, SpecLevel::Tone) => "spectrum dBFS",
            (LevelScale::DbSpl, SpecLevel::Tone) => "spectrum dB SPL",
            (LevelScale::Dbfs, SpecLevel::Density) => "spectrum dBFS/Hz",
            (LevelScale::DbSpl, SpecLevel::Density) => "spectrum dB SPL/Hz",
        },
        TraceKind::Rta {
            scale: LevelScale::Dbfs,
        } => "rta dBFS",
        TraceKind::Rta {
            scale: LevelScale::DbSpl,
        } => "rta dB SPL",
        TraceKind::Sweep => "sweep",
    }
}

pub(super) fn import_ac2(lines: &[(usize, &str)]) -> Result<Imported, ImportError> {
    let first = lines
        .iter()
        .find(|(_, l)| !l.trim().is_empty())
        .ok_or_else(|| fail(None, ImportProblem::NoData, "empty file"))?;
    let magic = first.1.trim();
    if ![
        AC2_CSV_MAGIC,
        AC2_CSV_MAGIC_V5,
        AC2_CSV_MAGIC_V4,
        AC2_CSV_MAGIC_V3,
        AC2_CSV_MAGIC_V2,
        AC2_CSV_MAGIC_V1,
    ]
    .contains(&magic)
    {
        let msg = if magic.starts_with(AC2_CSV_PREFIX) {
            format!(
                "{magic:?}: another ac2 CSV version; this build reads {AC2_CSV_MAGIC:?}, \
                 {AC2_CSV_MAGIC_V5:?}, {AC2_CSV_MAGIC_V4:?}, {AC2_CSV_MAGIC_V3:?}, {AC2_CSV_MAGIC_V2:?} and {AC2_CSV_MAGIC_V1:?}"
            )
        } else {
            format!("not an ac2 CSV file (expected {AC2_CSV_MAGIC:?})")
        };
        return Err(fail(Some(first.0), ImportProblem::BadHeader, msg));
    }
    let mut name = None;
    let mut grid = None;
    let mut kind = TraceKind::Transfer;
    let mut delay = None;
    let mut info: Option<SweepInfo> = None;
    let mut room: Option<RoomAcoustics> = None;
    let mut unresolved: Option<SweepUnresolved> = None;
    let mut ir_head: Option<(usize, IrAnnounce)> = None;
    let mut mic_curve = false;
    let mut header = None;
    let bad = |no: usize, what: &str, e: &dyn std::fmt::Display| {
        fail(Some(no), ImportProblem::BadHeader, format!("{what}: {e}"))
    };
    for &(no, l) in lines {
        let l = l.trim();
        if let Some(m) = l.strip_prefix("# ") {
            if let Some(v) = m.strip_prefix("name: ") {
                name = Some(v.to_owned());
            } else if let Some(v) = m.strip_prefix("grid: ") {
                let g = serde_json::from_str::<GridDef>(v).map_err(|e| bad(no, "grid", &e))?;
                // Before anything is formed on it: a crafted grid may hold no columns,
                // billions of them, or none at a finite frequency.
                crate::check::grid(&g).map_err(|e| bad(no, "grid", &e))?;
                grid = Some(g);
            } else if let Some(v) = m.strip_prefix("kind: ") {
                kind = kind_from_header(v).ok_or_else(|| {
                    fail(Some(no), ImportProblem::BadHeader, format!("kind {v:?}"))
                })?;
            } else if let Some(v) = m.strip_prefix("delay_ms: ") {
                let ms = number(v)
                    .filter(|d| d.is_finite())
                    .ok_or_else(|| bad(no, "delay_ms", &format!("{v:?} is not a number")))?;
                delay = Some(Seconds(ms / 1000.0));
            } else if let Some(v) = m.strip_prefix("sweep_info: ") {
                let i = serde_json::from_str(v).map_err(|e| bad(no, "sweep_info", &e))?;
                crate::check::sweep_info(&i).map_err(|e| bad(no, "sweep_info", &e))?;
                info = Some(i);
            } else if let Some(v) = m.strip_prefix("sweep_unresolved: ") {
                unresolved =
                    Some(serde_json::from_str(v).map_err(|e| bad(no, "sweep_unresolved", &e))?);
            } else if let Some(v) = m.strip_prefix("room_metrics: ") {
                room = Some(serde_json::from_str(v).map_err(|e| bad(no, "room_metrics", &e))?);
            } else if let Some(v) = m.strip_prefix("sweep_ir: ") {
                let h = serde_json::from_str(v).map_err(|e| bad(no, "sweep_ir", &e))?;
                let a = IrAnnounce::Sweep(h);
                a.check().map_err(|e| bad(no, "sweep_ir", &e))?;
                ir_head = Some((no, a));
            } else if let Some(v) = m.strip_prefix("transfer_ir: ") {
                let h = serde_json::from_str(v).map_err(|e| bad(no, "transfer_ir", &e))?;
                let a = IrAnnounce::Transfer(h);
                a.check().map_err(|e| bad(no, "transfer_ir", &e))?;
                ir_head = Some((no, a));
            } else if m.starts_with("mic_curve: ") {
                mic_curve = true;
            }
        } else if !l.is_empty() && header.is_none() {
            header = Some((no, split(l, Delim::Comma)));
            break;
        }
    }
    let (hno, header) =
        header.ok_or_else(|| fail(None, ImportProblem::NoData, "no column header"))?;
    let all: Vec<&str> = header.iter().map(String::as_str).collect();
    let base = all
        .iter()
        .position(|n| n.starts_with('h') || n.starts_with("thd"))
        .unwrap_or(all.len());
    let (names, extra) = all.split_at(base);
    let orders = distortion_columns(extra).ok_or_else(|| {
        fail(
            Some(hno),
            ImportProblem::BadHeader,
            format!("unknown columns {header:?}"),
        )
    })?;
    let layout = match names {
        ["freq_hz", "mag_db"] => positional(2),
        ["freq_hz", "mag_db", "phase_deg"] => positional(3),
        ["freq_hz", "mag_db", "phase_deg", "coherence"] => positional(4),
        ["freq_hz", "mag_db", "coherence"] => Layout {
            freq: 0,
            mag: 1,
            phase: None,
            coh: Some(2),
        },
        _ => {
            return Err(fail(
                Some(hno),
                ImportProblem::BadHeader,
                format!("unknown columns {header:?}"),
            ));
        }
    };
    // The impulse-response table, when there is one, follows the frequency rows.
    let ir_at = lines
        .iter()
        .find(|(n, l)| *n > hno && l.trim() == IR_HEADER)
        .map(|(n, _)| *n);
    let body: Vec<(usize, &str)> = lines
        .iter()
        .copied()
        .filter(|(n, _)| *n > hno && ir_at.is_none_or(|i| *n < i))
        .collect();
    let mut t = table(&body, Some(Delim::Comma))?;
    if t.rows[0].1.len() != all.len() {
        return Err(fail(
            Some(t.rows[0].0),
            ImportProblem::ColumnCount,
            "data rows do not match the column header",
        ));
    }
    t.layout = layout;
    let raw = validate(&t)?;
    let rows = raw.freqs.len();
    let column = |i: usize| -> Vec<f32> { t.rows.iter().map(|(_, r)| number32(&r[i])).collect() };
    let curve = |i: usize| DistortionCurve {
        level_db: column(i),
        floor_db: column(i + 1),
    };
    // Distortion levels share the frequency rows (`table` refused ±∞ text); a finite level
    // beyond what any math keeps finite is refused here.
    if let Some((no, _)) = t
        .rows
        .iter()
        .find(|(_, r)| r[base..].iter().any(|v| !crate::check::db_ok(number32(v))))
    {
        return Err(fail(
            Some(*no),
            ImportProblem::OutOfRange,
            "distortion level out of range",
        ));
    }
    let (orders, ref_orders) = orders;
    let curves = |at: usize, orders: &[u8]| -> Vec<HarmonicCurve> {
        orders
            .iter()
            .enumerate()
            .map(|(j, &order)| HarmonicCurve {
                order,
                curve: curve(at + 2 * j),
            })
            .collect()
    };
    let distortion = (!orders.is_empty()).then(|| {
        let thd_at = base + 2 * orders.len();
        (
            curves(base, &orders),
            curve(thd_at),
            (!ref_orders.is_empty()).then(|| curves(thd_at + 2, &ref_orders)),
        )
    });
    let ir = match (ir_head, ir_at) {
        (Some((no, h)), Some(at)) => {
            let fits = match h {
                IrAnnounce::Sweep(_) => kind == TraceKind::Sweep,
                IrAnnounce::Transfer(_) => kind == TraceKind::Transfer,
            };
            if !fits {
                return Err(fail(
                    Some(no),
                    ImportProblem::BadHeader,
                    format!("{} on a {} trace", h.key(), kind_header(kind)),
                ));
            }
            Some((h, ir_table(lines, at, h)?))
        }
        (Some((no, h)), None) => {
            return Err(fail(
                Some(no),
                ImportProblem::NoData,
                format!("{} announced but no {IR_HEADER:?} table follows", h.key()),
            ));
        }
        (None, Some(at)) => {
            return Err(fail(
                Some(at),
                ImportProblem::BadHeader,
                "an impulse-response table without its sweep_ir or transfer_ir line",
            ));
        }
        (None, None) => None,
    };
    let (sweep_ir, transfer_ir) = match ir {
        Some((IrAnnounce::Sweep(_), ir)) => (Some(ir), None),
        Some((IrAnnounce::Transfer(h), ir)) => (
            None,
            Some(TransferIr {
                sample_rate: h.sample_rate,
                ir,
            }),
        ),
        None => (None, None),
    };
    let known = grid.clone();
    let (grid, columns) = on_import_grid(raw, grid)?;
    let mut notes = Vec::new();
    if mic_curve {
        notes.push(ImportNote::MicCurveNotApplied);
    }
    let sweep = if kind == TraceKind::Sweep {
        match (distortion, info, sweep_ir) {
            // Distortion is kept only on the grid it was written on (it is not resampled).
            _ if known.as_ref() != Some(&grid) => {
                notes.push(ImportNote::SweepOffGrid);
                None
            }
            (Some((harmonics, thd, reference_harmonics)), Some(info), Some(ir)) => {
                Some(SweepData {
                    harmonics,
                    thd,
                    reference_harmonics,
                    ir,
                    info,
                    room,
                    unresolved,
                })
            }
            _ => {
                notes.push(ImportNote::SweepWithoutAnalysis);
                None
            }
        }
    } else {
        None
    };
    if kind == TraceKind::Sweep && sweep.is_none() {
        kind = TraceKind::Transfer;
    }
    Ok(Imported {
        name,
        format: ImportFormat::Ac2Csv,
        kind,
        grid,
        columns,
        rows,
        delay,
        sweep,
        ir: transfer_ir,
        notes,
        ir_file: None,
    })
}

/// The impulse-response table whose column header is line `at` (`t_s,linear,etc_db`).
/// Exactly `h.points` rows (the announcement was checked before: the vectors are sized
/// from it); finite samples, and an ETC finite or `-inf` (the dB of an all-zero stretch).
fn ir_table(lines: &[(usize, &str)], at: usize, a: IrAnnounce) -> Result<TraceIr, ImportError> {
    let h = a.table();
    a.check()
        .map_err(|e| fail(Some(at), ImportProblem::BadHeader, e))?;
    let mut linear = Vec::with_capacity(h.points as usize);
    let mut etc_db = Vec::with_capacity(h.points as usize);
    for &(no, l) in lines.iter().filter(|(n, _)| *n > at) {
        let l = l.trim();
        if l.is_empty() || is_comment(l) {
            continue;
        }
        let f: Vec<&str> = l.split(',').map(str::trim).collect();
        if f.len() != 3 {
            return Err(fail(
                Some(no),
                ImportProblem::ColumnCount,
                format!("{} columns, the impulse response has 3", f.len()),
            ));
        }
        let v = |s: &str| {
            s.parse::<f32>().map_err(|_| {
                fail(
                    Some(no),
                    ImportProblem::BadNumber,
                    format!("{s:?} is not a number"),
                )
            })
        };
        let (l, e) = (v(f[1])?, v(f[2])?);
        if !l.is_finite() || !(e.is_finite() || e == f32::NEG_INFINITY) {
            return Err(fail(
                Some(no),
                ImportProblem::OutOfRange,
                "impulse-response sample not finite or ETC NaN / +inf",
            ));
        }
        linear.push(l);
        etc_db.push(e);
        if linear.len() > TraceIr::MAX_POINTS as usize {
            return Err(fail(
                Some(no),
                ImportProblem::TooManyRows,
                format!("more than {} impulse-response rows", TraceIr::MAX_POINTS),
            ));
        }
    }
    if linear.len() != h.points as usize {
        return Err(fail(
            Some(at),
            ImportProblem::NoData,
            format!(
                "{} impulse-response rows, {} announces {}",
                linear.len(),
                a.key(),
                h.points
            ),
        ));
    }
    Ok(TraceIr {
        t0: h.t0,
        dt: h.dt,
        linear,
        etc_db,
    })
}

/// Orders of the distortion columns `h<k>_db,h<k>_floor_db,…,thd_db,thd_floor_db`, then of
/// the reference's own `ref_h<k>_db,ref_h<k>_floor_db,…` (none when `extra` is empty); `None`
/// when they are anything else.
fn distortion_columns(extra: &[&str]) -> Option<(Vec<u8>, Vec<u8>)> {
    if extra.is_empty() {
        return Some((Vec::new(), Vec::new()));
    }
    let thd = extra.iter().position(|n| *n == "thd_db")?;
    let (pairs, rest) = extra.split_at(thd);
    let refs = rest.strip_prefix(&["thd_db", "thd_floor_db"][..])?;
    Some((order_pairs(pairs, "h")?, order_pairs(refs, "ref_h")?))
}

/// Orders of `<prefix><k>_db,<prefix><k>_floor_db` pairs.
fn order_pairs(pairs: &[&str], prefix: &str) -> Option<Vec<u8>> {
    if !pairs.len().is_multiple_of(2) {
        return None;
    }
    pairs
        .chunks(2)
        .map(|p| {
            let k: u8 = p[0]
                .strip_prefix(prefix)?
                .strip_suffix("_db")?
                .parse()
                .ok()?;
            (p[1] == format!("{prefix}{k}_floor_db") && k >= 2).then_some(k)
        })
        .collect()
}
