//! Text formats: ac2 CSV (export and re-import) and analyzer text exports (import).
//!
//! # ac2 CSV
//!
//! ```text
//! # ac2 trace export v3
//! # name: Main L pre EQ
//! # kind: transfer
//! # source: captured from "main-l" (measurement 1), session epoch 3, sample 480000
//! # delay_ms: 12.5
//! # …                      (every metadata field, one per line)
//! # grid: {"type":"log","ppo":48,"k_min":-240,"k_max":239}
//! freq_hz,mag_db,phase_deg,coherence
//! 19.99700,-3.25,45.5,0.98
//! ```
//!
//! Columns are as measured: offset, polarity, nudge, smoothing and a mic curve applied after
//! capture are display edits, listed in the header but not applied. `phase_deg` /
//! `coherence` are present only when the trace has them. A gap is `nan`. Values are written
//! in their shortest exact form, so an export re-imports bit-for-bit onto the grid named in
//! the header. The import keeps the name, kind and `delay_ms` (the delay the phase is
//! referred to); the other display edits start fresh.
//!
//! A sweep trace adds `h2_db,h2_floor_db,…,thd_db,thd_floor_db` after `phase_deg`: each
//! harmonic order's level and noise floor in dB re the fundamental at the row's (fundamental)
//! frequency, then the total. Its analysis facts are one JSON header line
//! (`# sweep_info: {…}`, [`SweepInfo`]), its decimated impulse response a second table after
//! the frequency rows, announced by `# sweep_ir: {"t0":…,"dt":…,"points":n}`:
//!
//! ```text
//! t_s,linear,etc_db
//! -0.0021,0.0003,-71.5
//! ```
//!
//! (`t_s` = `t0 + i·dt` re the arrival, written for reading; the import uses `t0`/`dt`).
//! Its ISO 3382-1 room parameters are one JSON header line (`# room_metrics: {…}`,
//! [`RoomAcoustics`]) and, for reading, a table of comment lines after the impulse
//! response (`# band_hz,edt_s,…`; a refused value reads `refused:<why>`).
//! Such an export re-imports as the sweep trace it was. A version 2 export (written before
//! the room parameters were exported) imports as a sweep without them. A version 1 export
//! (written before the analysis facts were exported) is still read: a sweep among them
//! imports as its transfer function with the distortion dropped and
//! [`ImportNote::SweepWithoutAnalysis`] on the trace.
//!
//! # Analyzer text
//!
//! Liberal in what it reads: columns separated by commas, semicolons (decimal commas
//! allowed), tabs or spaces; comment lines starting with `#`, `*`, `;`, `%`, `!` or `//`;
//! header lines before the first data row (mapped by name when they name the columns:
//! freq, mag/SPL/dB/level, phase/deg, coh); otherwise columns are positional
//! `freq mag [phase] [coherence]`. Coherence in percent (most values above 1) is scaled to 0…1.
//! Strict in what it accepts: frequencies positive and strictly ascending, every row with
//! the same column count, at least two finite magnitudes. Data off a known grid is
//! resampled onto a log grid (linear in dB over log frequency).

use std::fmt::Write as _;

use ac2_proto::GridDef;
use ac2_proto::ImportProblem;
use ac2_proto::frame::MAX_N;
use ac2_proto::model::{
    CalState, DepthPolicy, DistortionCurve, HarmonicCurve, ImportFormat, ImportNote, ImportRole,
    MicState, Polarity, RoomAcoustics, RoomRefusal, RoomValue, SmoothingFraction, SmoothingMode,
    SweepData, SweepInfo, SweepIr, TraceKind, TraceMicCurve, TraceSource,
};
use ac2_proto::units::Seconds;
use serde::{Deserialize, Serialize};

use crate::columns::{Columns, StoredTrace, frequencies, resample, wrap_deg};

/// First line of an ac2 CSV file of this format version.
pub const AC2_CSV_MAGIC: &str = "# ac2 trace export v3";
/// First line of the previous version, still read: it has no room parameters (and nothing
/// else differs).
const AC2_CSV_MAGIC_V2: &str = "# ac2 trace export v2";
/// First line of the version before, still read: it has no sweep analysis facts and no
/// impulse response either.
const AC2_CSV_MAGIC_V1: &str = "# ac2 trace export v1";
const AC2_CSV_PREFIX: &str = "# ac2 trace export";
/// Column header of a sweep export's impulse-response table.
const IR_HEADER: &str = "t_s,linear,etc_db";

/// The `# sweep_ir:` header line: where the impulse response's points sit in time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IrHeader {
    t0: Seconds,
    dt: Seconds,
    points: u32,
}

/// A refused import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    /// 1-based line, when the problem has one.
    pub line: Option<u32>,
    /// Problem.
    pub problem: ImportProblem,
    /// Human-readable detail.
    pub msg: String,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(l) => write!(f, "line {l}: {}", self.msg),
            None => f.write_str(&self.msg),
        }
    }
}

impl std::error::Error for ImportError {}

fn fail(line: Option<usize>, problem: ImportProblem, msg: impl Into<String>) -> ImportError {
    ImportError {
        line: line.map(|l| u32::try_from(l).unwrap_or(u32::MAX)),
        problem,
        msg: msg.into(),
    }
}

/// A parsed file, on its grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// Name from an ac2 CSV header.
    pub name: Option<String>,
    /// Format actually parsed.
    pub format: ImportFormat,
    /// Kind (`target` for the target role; else from an ac2 header, else transfer).
    pub kind: TraceKind,
    /// Grid of `columns`.
    pub grid: GridDef,
    /// Data.
    pub columns: Columns,
    /// Data rows read.
    pub rows: usize,
    /// Delay the phase is referred to (an ac2 CSV's `delay_ms`); `None` for files that do
    /// not state one.
    pub delay: Option<Seconds>,
    /// A sweep export's distortion, impulse response and analysis facts (`kind` is then
    /// `sweep`).
    pub sweep: Option<SweepData>,
    /// What the file held that the import does not keep.
    pub notes: Vec<ImportNote>,
}

fn decode(content: &[u8]) -> Result<String, ImportError> {
    if content.contains(&0) {
        return Err(fail(None, ImportProblem::NotText, "not a text file"));
    }
    let s = match std::str::from_utf8(content) {
        Ok(s) => s.to_owned(),
        // Older analyzers write Windows-1252 / Latin-1 (`°` in headers); every byte maps.
        Err(_) => content.iter().map(|b| char::from(*b)).collect(),
    };
    Ok(s.trim_start_matches('\u{feff}').to_owned())
}

fn is_comment(l: &str) -> bool {
    ["#", "*", ";", "%", "!", "//"]
        .iter()
        .any(|p| l.starts_with(p))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Delim {
    Semicolon,
    Tab,
    Comma,
    Space,
}

fn number(s: &str) -> Option<f64> {
    let t = s.trim().trim_matches('"').trim();
    if t.eq_ignore_ascii_case("nan") {
        return Some(f64::NAN);
    }
    let v: f64 = t.parse().ok()?;
    // `inf` parses but is never a measured value.
    (!v.is_infinite()).then_some(v)
}

/// [`number`] parsed straight to f32 (already checked by [`number`]).
fn number32(s: &str) -> f32 {
    let t = s.trim().trim_matches('"').trim();
    if t.eq_ignore_ascii_case("nan") {
        return f32::NAN;
    }
    t.parse().unwrap_or(f32::NAN)
}

fn split(l: &str, d: Delim) -> Vec<String> {
    let fields: Vec<String> = match d {
        Delim::Semicolon => l.split(';').map(|f| f.replace(',', ".")).collect(),
        Delim::Tab => l.split('\t').map(|f| f.replace(',', ".")).collect(),
        Delim::Comma => l.split(',').map(str::to_owned).collect(),
        Delim::Space => l.split_whitespace().map(|f| f.replace(',', ".")).collect(),
    };
    let mut fields: Vec<String> = fields.into_iter().map(|f| f.trim().to_owned()).collect();
    // A trailing separator leaves an empty last field.
    while fields.last().is_some_and(String::is_empty) {
        fields.pop();
    }
    fields
}

/// The delimiter that splits `l` into at least two numbers, if any.
fn detect(l: &str) -> Option<Delim> {
    let numeric = |d| {
        let f = split(l, d);
        f.len() >= 2 && f.iter().all(|x| number(x).is_some())
    };
    let mut order = Vec::new();
    if l.contains(';') {
        order.push(Delim::Semicolon);
    }
    if l.contains('\t') {
        order.push(Delim::Tab);
    }
    if l.contains(',') {
        order.push(Delim::Comma);
    }
    order.push(Delim::Space);
    order.into_iter().find(|d| numeric(*d))
}

/// Column roles.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    freq: usize,
    mag: usize,
    phase: Option<usize>,
    coh: Option<usize>,
}

fn positional(n: usize) -> Layout {
    Layout {
        freq: 0,
        mag: 1,
        phase: (n > 2).then_some(2),
        coh: (n > 3).then_some(3),
    }
}

/// Maps a header row onto columns by name; `None` unless it names frequency and magnitude.
fn named(header: &[String]) -> Option<Layout> {
    let find = |pred: &dyn Fn(&str) -> bool| {
        header
            .iter()
            .position(|h| pred(&h.to_lowercase().replace('"', "")))
    };
    let freq = find(&|h| h.contains("freq") || h == "hz" || h == "f")?;
    let coh = find(&|h| h.contains("coh"));
    let phase = find(&|h| h.contains("phase") || h.contains("deg"));
    let mag = find(&|h| {
        ["mag", "spl", "db", "level", "gain", "amp", "response"]
            .iter()
            .any(|k| h.contains(k))
            && !h.contains("phase")
            && !h.contains("freq")
    })?;
    let distinct = [Some(freq), Some(mag), phase, coh];
    for (i, a) in distinct.iter().enumerate() {
        if a.is_some() && distinct[..i].contains(a) {
            return None;
        }
    }
    Some(Layout {
        freq,
        mag,
        phase,
        coh,
    })
}

struct Table {
    /// Data rows as checked numeric fields; parsed per column type later, so magnitudes go
    /// straight to f32 (one rounding, exact for what `export_csv` wrote).
    rows: Vec<(usize, Vec<String>)>,
    layout: Layout,
}

/// Reads the numeric table of `lines` (1-based line numbers).
fn table(lines: &[(usize, &str)], delim: Option<Delim>) -> Result<Table, ImportError> {
    let mut header: Option<Vec<String>> = None;
    let mut delim = delim;
    let mut rows: Vec<(usize, Vec<String>)> = Vec::new();
    let mut width = 0usize;
    for &(no, l) in lines {
        let l = l.trim();
        if l.is_empty() || is_comment(l) {
            continue;
        }
        if rows.is_empty() {
            let d = match delim {
                Some(d) => {
                    let f = split(l, d);
                    (f.len() >= 2 && f.iter().all(|x| number(x).is_some())).then_some(d)
                }
                None => detect(l),
            };
            let Some(d) = d else {
                // Before the first data row a non-numeric line is a header.
                header = Some(
                    [Delim::Semicolon, Delim::Tab, Delim::Comma]
                        .into_iter()
                        .find(|d| split(l, *d).len() >= 2)
                        .map_or_else(|| split(l, Delim::Space), |d| split(l, d)),
                );
                continue;
            };
            delim = Some(d);
        }
        let Some(d) = delim else { continue };
        let fields = split(l, d);
        if rows.is_empty() {
            width = fields.len();
        } else if fields.len() != width {
            return Err(fail(
                Some(no),
                ImportProblem::ColumnCount,
                format!("{} columns, the first data row has {width}", fields.len()),
            ));
        }
        if let Some(f) = fields.iter().find(|f| number(f).is_none()) {
            return Err(fail(
                Some(no),
                ImportProblem::BadNumber,
                format!("{f:?} is not a number"),
            ));
        }
        rows.push((no, fields));
        if rows.len() > MAX_N as usize {
            return Err(fail(
                Some(no),
                ImportProblem::TooManyRows,
                format!("more than {MAX_N} rows"),
            ));
        }
    }
    if rows.is_empty() {
        return Err(fail(None, ImportProblem::NoData, "no data rows"));
    }
    if width < 2 {
        return Err(fail(
            Some(rows[0].0),
            ImportProblem::TooFewColumns,
            "frequency and magnitude columns are required",
        ));
    }
    let layout = header
        .as_deref()
        .filter(|h| h.len() == width)
        .and_then(named)
        .unwrap_or_else(|| positional(width));
    Ok(Table { rows, layout })
}

struct Raw {
    freqs: Vec<f64>,
    columns: Columns,
}

fn validate(t: &Table) -> Result<Raw, ImportError> {
    let lay = t.layout;
    let mut freqs = Vec::with_capacity(t.rows.len());
    let mut mag = Vec::with_capacity(t.rows.len());
    let mut phase = lay.phase.map(|_| Vec::with_capacity(t.rows.len()));
    let mut coh = lay.coh.map(|_| Vec::with_capacity(t.rows.len()));
    for (no, r) in &t.rows {
        let f = number(&r[lay.freq]).unwrap_or(f64::NAN);
        // 0 Hz is accepted as the first row only (the DC bin of an FFT grid).
        if !(f.is_finite() && (f > 0.0 || (f == 0.0 && freqs.is_empty()))) {
            return Err(fail(
                Some(*no),
                ImportProblem::OutOfRange,
                format!("frequency {f} is not a positive number"),
            ));
        }
        if freqs.last().is_some_and(|p| f <= *p) {
            return Err(fail(
                Some(*no),
                ImportProblem::NotAscending,
                format!("frequency {f} Hz does not ascend"),
            ));
        }
        freqs.push(f);
        let m = number32(&r[lay.mag]);
        if m.is_infinite() {
            return Err(fail(
                Some(*no),
                ImportProblem::OutOfRange,
                "magnitude out of range",
            ));
        }
        mag.push(m);
        if let (Some(p), Some(i)) = (phase.as_mut(), lay.phase) {
            let v = number32(&r[i]);
            // In-range values pass through exactly; unwrapped exports are wrapped.
            p.push(if !v.is_finite() {
                f32::NAN
            } else if v > -180.0 && v <= 180.0 {
                v
            } else {
                wrap_deg(f64::from(v)) as f32
            });
        }
        if let (Some(c), Some(i)) = (coh.as_mut(), lay.coh) {
            c.push(number32(&r[i]));
        }
    }
    if mag.iter().filter(|v| v.is_finite()).count() < 2 {
        return Err(fail(
            None,
            ImportProblem::NoData,
            "fewer than two rows with a magnitude",
        ));
    }
    let coherence = match coh {
        None => None,
        Some(c) => {
            // Percent when most values are above 1 (a coherence in 0…1 never is).
            let finite = c.iter().filter(|v| v.is_finite()).count();
            let above = c.iter().filter(|v| v.is_finite() && **v > 1.0).count();
            let percent = above * 2 > finite;
            let mut out = Vec::with_capacity(c.len());
            for (v, (no, _)) in c.iter().zip(&t.rows) {
                let s = if percent { v / 100.0 } else { *v };
                if s.is_finite() && !(0.0..=1.0).contains(&s) {
                    return Err(fail(
                        Some(*no),
                        ImportProblem::BadCoherence,
                        format!("coherence {v} is outside 0…1 (or 0…100 %)"),
                    ));
                }
                out.push(s);
            }
            Some(out)
        }
    };
    Ok(Raw {
        freqs,
        columns: Columns {
            mag_db: mag,
            phase_deg: phase,
            coherence,
        },
    })
}

/// Points per octave of the log grid an off-grid import is resampled onto: 48 (the
/// default measurement grid, so A−B against a capture needs no second resampling), 96
/// when the file is denser than that.
fn import_grid(freqs: &[f64]) -> Result<GridDef, ImportError> {
    let lo = freqs.iter().copied().find(|f| *f > 0.0).unwrap_or(1.0);
    let hi = freqs[freqs.len() - 1];
    let octaves = (hi / lo).log2();
    let density = if octaves > 0.0 {
        (freqs.len() - 1) as f64 / octaves
    } else {
        0.0
    };
    let ppo: u32 = if density > 48.0 { 96 } else { 48 };
    let p = f64::from(ppo);
    // A small tolerance keeps a file written on this very grid on it.
    let k_min = (p * (lo / 1000.0).log2() - 1e-6).ceil() as i32;
    let k_max = (p * (hi / 1000.0).log2() + 1e-6).floor() as i32;
    if k_max <= k_min {
        return Err(fail(
            None,
            ImportProblem::NoData,
            format!("the data spans less than two columns of a 1/{ppo}-octave grid"),
        ));
    }
    Ok(GridDef::Log { ppo, k_min, k_max })
}

fn on_import_grid(raw: Raw, known: Option<GridDef>) -> Result<(GridDef, Columns), ImportError> {
    if let Some(g) = known {
        let gf = frequencies(&g);
        let same = gf.len() == raw.freqs.len()
            && gf
                .iter()
                .zip(&raw.freqs)
                .all(|(a, b)| (a - b).abs() <= 1e-9 * a.abs().max(1.0));
        if same {
            return Ok((g, raw.columns));
        }
    }
    let g = import_grid(&raw.freqs)?;
    let c = resample(&raw.freqs, &raw.columns, &frequencies(&g));
    Ok((g, c))
}

fn kind_from_header(v: &str) -> Option<TraceKind> {
    use ac2_proto::model::LevelScale;
    Some(match v.trim() {
        "transfer" => TraceKind::Transfer,
        "target" => TraceKind::Target,
        "spectrum dBFS" => TraceKind::Spectrum {
            scale: LevelScale::Dbfs,
        },
        "spectrum dB SPL" => TraceKind::Spectrum {
            scale: LevelScale::DbSpl,
        },
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

fn kind_header(k: TraceKind) -> &'static str {
    use ac2_proto::model::LevelScale;
    match k {
        TraceKind::Transfer => "transfer",
        TraceKind::Target => "target",
        TraceKind::Spectrum {
            scale: LevelScale::Dbfs,
        } => "spectrum dBFS",
        TraceKind::Spectrum {
            scale: LevelScale::DbSpl,
        } => "spectrum dB SPL",
        TraceKind::Rta {
            scale: LevelScale::Dbfs,
        } => "rta dBFS",
        TraceKind::Rta {
            scale: LevelScale::DbSpl,
        } => "rta dB SPL",
        TraceKind::Sweep => "sweep",
    }
}

fn import_ac2(lines: &[(usize, &str)]) -> Result<Imported, ImportError> {
    let first = lines
        .iter()
        .find(|(_, l)| !l.trim().is_empty())
        .ok_or_else(|| fail(None, ImportProblem::NoData, "empty file"))?;
    let magic = first.1.trim();
    if ![AC2_CSV_MAGIC, AC2_CSV_MAGIC_V2, AC2_CSV_MAGIC_V1].contains(&magic) {
        let msg = if magic.starts_with(AC2_CSV_PREFIX) {
            format!(
                "{magic:?}: another ac2 CSV version; this build reads {AC2_CSV_MAGIC:?}, \
                 {AC2_CSV_MAGIC_V2:?} and {AC2_CSV_MAGIC_V1:?}"
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
    let mut ir_head: Option<(usize, IrHeader)> = None;
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
                grid = Some(serde_json::from_str::<GridDef>(v).map_err(|e| bad(no, "grid", &e))?);
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
                info = Some(serde_json::from_str(v).map_err(|e| bad(no, "sweep_info", &e))?);
            } else if let Some(v) = m.strip_prefix("room_metrics: ") {
                room = Some(serde_json::from_str(v).map_err(|e| bad(no, "room_metrics", &e))?);
            } else if let Some(v) = m.strip_prefix("sweep_ir: ") {
                ir_head = Some((
                    no,
                    serde_json::from_str(v).map_err(|e| bad(no, "sweep_ir", &e))?,
                ));
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
    let distortion = (!orders.is_empty()).then(|| {
        let harmonics = orders
            .iter()
            .enumerate()
            .map(|(j, &order)| HarmonicCurve {
                order,
                curve: curve(base + 2 * j),
            })
            .collect();
        (harmonics, curve(base + 2 * orders.len()))
    });
    let ir = match (ir_head, ir_at) {
        (Some((_, h)), Some(at)) => Some(ir_table(lines, at, h)?),
        (Some((no, _)), None) => {
            return Err(fail(
                Some(no),
                ImportProblem::NoData,
                format!("sweep_ir announced but no {IR_HEADER:?} table follows"),
            ));
        }
        (None, Some(at)) => {
            return Err(fail(
                Some(at),
                ImportProblem::BadHeader,
                "an impulse-response table without its sweep_ir line",
            ));
        }
        (None, None) => None,
    };
    let known = grid.clone();
    let (grid, columns) = on_import_grid(raw, grid)?;
    let mut notes = Vec::new();
    if mic_curve {
        notes.push(ImportNote::MicCurveNotApplied);
    }
    let sweep = if kind == TraceKind::Sweep {
        match (distortion, info, ir) {
            // Distortion is kept only on the grid it was written on (it is not resampled).
            _ if known.as_ref() != Some(&grid) => {
                notes.push(ImportNote::SweepOffGrid);
                None
            }
            (Some((harmonics, thd)), Some(info), Some(ir)) => Some(SweepData {
                harmonics,
                thd,
                ir,
                info,
                room,
            }),
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
        notes,
    })
}

/// The impulse-response table whose column header is line `at` (`t_s,linear,etc_db`).
/// Exactly `h.points` rows; `inf` is accepted here, as a dB of an all-zero stretch.
fn ir_table(lines: &[(usize, &str)], at: usize, h: IrHeader) -> Result<SweepIr, ImportError> {
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
        linear.push(v(f[1])?);
        etc_db.push(v(f[2])?);
        if linear.len() > MAX_N as usize {
            return Err(fail(
                Some(no),
                ImportProblem::TooManyRows,
                format!("more than {MAX_N} impulse-response rows"),
            ));
        }
    }
    if linear.len() != h.points as usize {
        return Err(fail(
            Some(at),
            ImportProblem::NoData,
            format!(
                "{} impulse-response rows, sweep_ir announces {}",
                linear.len(),
                h.points
            ),
        ));
    }
    Ok(SweepIr {
        t0: h.t0,
        dt: h.dt,
        linear,
        etc_db,
    })
}

/// Orders of the distortion columns `h<k>_db,h<k>_floor_db,…,thd_db,thd_floor_db` (none
/// when `extra` is empty); `None` when they are anything else.
fn distortion_columns(extra: &[&str]) -> Option<Vec<u8>> {
    if extra.is_empty() {
        return Some(Vec::new());
    }
    let (pairs, thd) = extra.split_at(extra.len().checked_sub(2)?);
    if thd != ["thd_db", "thd_floor_db"] || pairs.len() % 2 != 0 {
        return None;
    }
    pairs
        .chunks(2)
        .map(|p| {
            let k: u8 = p[0].strip_prefix('h')?.strip_suffix("_db")?.parse().ok()?;
            (p[1] == format!("h{k}_floor_db") && k >= 2).then_some(k)
        })
        .collect()
}

fn import_text(lines: &[(usize, &str)]) -> Result<Imported, ImportError> {
    let t = table(lines, None)?;
    let raw = validate(&t)?;
    let rows = raw.freqs.len();
    let (grid, columns) = on_import_grid(raw, None)?;
    Ok(Imported {
        name: None,
        format: ImportFormat::AnalyzerText,
        kind: TraceKind::Transfer,
        grid,
        columns,
        rows,
        delay: None,
        sweep: None,
        notes: Vec::new(),
    })
}

/// Parses an imported file. A target keeps the magnitude only; a sweep export with its
/// analysis facts and impulse response is a sweep again, without them its transfer
/// function ([`Imported::notes`] says why).
pub fn import(
    content: &[u8],
    format: ImportFormat,
    role: ImportRole,
) -> Result<Imported, ImportError> {
    let text = decode(content)?;
    let lines: Vec<(usize, &str)> = text.lines().enumerate().map(|(i, l)| (i + 1, l)).collect();
    let ac2 = || {
        lines
            .iter()
            .find(|(_, l)| !l.trim().is_empty())
            .is_some_and(|(_, l)| l.trim().starts_with(AC2_CSV_PREFIX))
    };
    let mut imported = match format {
        ImportFormat::Ac2Csv => import_ac2(&lines)?,
        ImportFormat::AnalyzerText => import_text(&lines)?,
        ImportFormat::Auto if ac2() => import_ac2(&lines)?,
        ImportFormat::Auto => import_text(&lines)?,
    };
    if role == ImportRole::Target {
        // A target is a specified magnitude: nothing measured comes with it.
        imported.kind = TraceKind::Target;
        imported.columns.phase_deg = None;
        imported.columns.coherence = None;
        imported.sweep = None;
        imported.delay = None;
        imported.notes.clear();
    } else if imported.kind == TraceKind::Target {
        // A target exported and re-imported as a trace is a magnitude-only transfer curve.
        imported.kind = TraceKind::Transfer;
    }
    Ok(imported)
}

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
        } => {
            if notes.is_empty() {
                format!("imported from {file_name:?} ({format:?})")
            } else {
                format!("imported from {file_name:?} ({format:?}), notes {notes:?}")
            }
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
            reference_input,
            measurement_input,
        } => format!(
            "run {number} of sweep measurement {meas_name:?} (measurement {meas}, sweep {}; {} Hz – {} Hz, {} s, {} dBFS, {repeats}×), input {} re {}, session epoch {}",
            run.0,
            sweep.start.0,
            sweep.end.0,
            sweep.duration.0,
            level.0,
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
        let h = IrHeader {
            t0: sw.ir.t0,
            dt: sw.ir.dt,
            points: u32::try_from(sw.ir.linear.len().min(sw.ir.etc_db.len())).unwrap_or(u32::MAX),
        };
        line(
            "sweep_ir",
            serde_json::to_string(&h).unwrap_or_else(|_| "null".into()),
        );
        if let Some(r) = &sw.room {
            line(
                "room_metrics",
                serde_json::to_string(r).unwrap_or_else(|_| "null".into()),
            );
        }
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
            .collect()
    });
    if let Some(s) = &t.sweep {
        for h in &s.harmonics {
            let _ = write!(out, ",h{0}_db,h{0}_floor_db", h.order);
        }
        out.push_str(",thd_db,thd_floor_db");
    }
    out.push('\n');
    for (i, f) in frequencies(&t.grid).iter().enumerate() {
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
        out.push_str(IR_HEADER);
        out.push('\n');
        for (i, (l, e)) in sw.ir.linear.iter().zip(&sw.ir.etc_db).enumerate() {
            let at = sw.ir.t0.0 + i as f64 * sw.ir.dt.0;
            let _ = writeln!(out, "{at},{},{}", num32(*l), num32(*e));
        }
        if let Some(r) = &sw.room {
            room_table(&mut out, r);
        }
    }
    out
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn delimiters() {
        assert_eq!(detect("20,-3.5,10"), Some(Delim::Comma));
        assert_eq!(detect("20;-3,5;10"), Some(Delim::Semicolon));
        assert_eq!(detect("20\t-3.5\t10"), Some(Delim::Tab));
        assert_eq!(detect("  20   -3.5 10 "), Some(Delim::Space));
        assert_eq!(detect("20 -3,5 10"), Some(Delim::Space));
        assert_eq!(detect("Freq(Hz) SPL(dB)"), None);
    }

    #[test]
    fn header_names() {
        let h = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            named(&h(&[
                "Frequency (Hz)",
                "Coherence",
                "Magnitude (dB)",
                "Phase (deg)"
            ])),
            Some(Layout {
                freq: 0,
                mag: 2,
                phase: Some(3),
                coh: Some(1),
            })
        );
        assert_eq!(named(&h(&["a", "b"])), None);
    }
}
