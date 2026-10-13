//! Text formats: ac2 CSV (export and re-import) and analyzer text exports (import).
//!
//! # ac2 CSV
//!
//! ```text
//! # ac2 trace export v6
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
//! frequency, then the total, then the reference input's own orders
//! (`ref_h2_db,ref_h2_floor_db,…`, absent from an export written without them). Its analysis facts are one JSON header line
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
//! Where its columns are finer than its windows resolve is one JSON header line
//! (`# sweep_unresolved: {…}`, [`SweepUnresolved`]).
//! Such an export re-imports as the sweep trace it was.
//!
//! A transfer trace captured with an impulse response ([`TransferIr`]) writes it the same
//! way: `# transfer_ir: {"sample_rate":…,"t0":…,"dt":…,"points":n}` and the same
//! `t_s,linear,etc_db` table after the frequency rows, `t_s` re the trace's `delay_ms` (the
//! inserted delay at capture). It re-imports with the trace.
//!
//! A version 5 export (written before sweeps kept the reference input's own harmonics)
//! imports as a sweep without them. A version 4 export (written before sweeps said where their columns are finer than
//! their windows resolve) imports as a sweep without that. A version 3 export (written before transfer traces kept their impulse response) imports
//! as it was. A version 2 export (written before
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
//!
//! [`SweepInfo`]: ac2_proto::model::SweepInfo
//! [`RoomAcoustics`]: ac2_proto::model::RoomAcoustics
//! [`SweepUnresolved`]: ac2_proto::model::SweepUnresolved

mod ac2;
mod export;
mod table;

pub use ac2::header_kind;
pub use export::export_csv;

use ac2_proto::GridDef;
use ac2_proto::ImportProblem;
use ac2_proto::model::{
    ImportFormat, ImportNote, ImportRole, SweepData, TraceIr, TraceKind, TransferIr,
};
use ac2_proto::units::{Hz, Seconds};
use serde::{Deserialize, Serialize};

use crate::columns::Columns;

use ac2::import_ac2;
use table::{on_import_grid, table, validate};

/// First line of an ac2 CSV file of this format version.
pub const AC2_CSV_MAGIC: &str = "# ac2 trace export v6";
/// First line of the previous version, still read: no sweep in it has the reference
/// input's own harmonics (and nothing else differs).
const AC2_CSV_MAGIC_V5: &str = "# ac2 trace export v5";
/// First line of the previous version, still read: no sweep in it says where its columns
/// are finer than its windows resolve (and nothing else differs).
const AC2_CSV_MAGIC_V4: &str = "# ac2 trace export v4";
/// First line of the previous version, still read: no transfer trace in it has an impulse
/// response (and nothing else differs).
const AC2_CSV_MAGIC_V3: &str = "# ac2 trace export v3";
/// First line of the version before, still read: it has no room parameters either.
const AC2_CSV_MAGIC_V2: &str = "# ac2 trace export v2";
/// First line of the version before, still read: it has no sweep analysis facts and no
/// impulse response either.
const AC2_CSV_MAGIC_V1: &str = "# ac2 trace export v1";
const AC2_CSV_PREFIX: &str = "# ac2 trace export";
/// Column header of an export's impulse-response table.
const IR_HEADER: &str = "t_s,linear,etc_db";

/// The `# sweep_ir:` header line: where the impulse response's points sit in time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IrHeader {
    t0: Seconds,
    dt: Seconds,
    points: u32,
}

impl IrHeader {
    fn of(ir: &TraceIr) -> Self {
        Self {
            t0: ir.t0,
            dt: ir.dt,
            points: u32::try_from(ir.linear.len().min(ir.etc_db.len())).unwrap_or(u32::MAX),
        }
    }
}

/// The `# transfer_ir:` header line: a transfer trace's impulse response, with the sample
/// rate it was measured at.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferIrHeader {
    sample_rate: Hz,
    t0: Seconds,
    dt: Seconds,
    points: u32,
}

/// Which header line announced an impulse-response table.
#[derive(Debug, Clone, Copy, PartialEq)]
enum IrAnnounce {
    Sweep(IrHeader),
    Transfer(TransferIrHeader),
}

impl IrAnnounce {
    fn key(self) -> &'static str {
        match self {
            Self::Sweep(_) => "sweep_ir",
            Self::Transfer(_) => "transfer_ir",
        }
    }

    /// The announced table's length and timing, checked before the table is read (its
    /// vectors are sized from `points`), and a transfer IR's sample rate.
    fn check(self) -> Result<(), String> {
        let h = self.table();
        crate::check::ir_points(h.points)?;
        crate::check::ir_timing(h.t0.0, h.dt.0)?;
        if let Self::Transfer(t) = self
            && !(t.sample_rate.0.is_finite() && t.sample_rate.0 > 0.0)
        {
            return Err("impulse response sample rate must be finite and above 0".into());
        }
        Ok(())
    }

    fn table(self) -> IrHeader {
        match self {
            Self::Sweep(h) => h,
            Self::Transfer(h) => IrHeader {
                t0: h.t0,
                dt: h.dt,
                points: h.points,
            },
        }
    }
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

pub(crate) fn fail(
    line: Option<usize>,
    problem: ImportProblem,
    msg: impl Into<String>,
) -> ImportError {
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
    /// A transfer export's impulse response (`kind` is then `transfer`).
    pub ir: Option<TransferIr>,
    /// What the file held that the import does not keep.
    pub notes: Vec<ImportNote>,
    /// An impulse-response file's facts ([`crate::ir_wav`]).
    pub ir_file: Option<Box<ac2_proto::model::IrFile>>,
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
        ir: None,
        notes: Vec::new(),
        ir_file: None,
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
        ImportFormat::IrWav => {
            return Err(fail(
                None,
                ImportProblem::NotText,
                "an impulse-response WAV file is not text (crate::import_file reads it)",
            ));
        }
    };
    if role == ImportRole::Target {
        // A target is a specified magnitude: nothing measured comes with it.
        imported.kind = TraceKind::Target;
        imported.columns.phase_deg = None;
        imported.columns.coherence = None;
        imported.sweep = None;
        imported.ir = None;
        imported.delay = None;
        imported.notes.clear();
    } else if imported.kind == TraceKind::Target {
        // A target exported and re-imported as a trace is a magnitude-only transfer curve.
        imported.kind = TraceKind::Transfer;
    }
    Ok(imported)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::ac2::{kind_from_header, kind_header};
    use super::table::{Delim, Layout, detect, named};
    use super::*;
    use ac2_proto::model::{LevelScale, SpecLevel};

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

    #[test]
    fn spectrum_kinds_name_their_scale_and_level() {
        for scale in [LevelScale::Dbfs, LevelScale::DbSpl] {
            for level in [SpecLevel::Tone, SpecLevel::Density] {
                let k = TraceKind::Spectrum { scale, level };
                assert_eq!(kind_from_header(kind_header(k)), Some(k), "{k:?}");
            }
        }
        // Files of before the level choice read tone, what their spectra held.
        assert_eq!(
            kind_from_header("spectrum dB SPL"),
            Some(TraceKind::Spectrum {
                scale: LevelScale::DbSpl,
                level: SpecLevel::Tone
            })
        );
        assert_eq!(
            kind_header(TraceKind::Spectrum {
                scale: LevelScale::DbSpl,
                level: SpecLevel::Density
            }),
            "spectrum dB SPL/Hz"
        );
    }
}
