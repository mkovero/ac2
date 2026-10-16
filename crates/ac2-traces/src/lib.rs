//! Stored traces outside the live DSP path: the column data a trace holds, capture from a
//! published frame, averaging, math channels' combinations, the text formats traces are imported from and
//! exported to, and the session directory format.
//!
//! The daemon owns the trace store and calls into this crate; the client's fake daemon uses
//! the same code so tests against it see real numbers. Display edits (offset, polarity,
//! nudge, smoothing, a mic curve applied after capture) are never baked into stored
//! columns: columns stay as measured, and the display smoothing and mic curve are applied
//! only when a trace's data is served ([`smooth`], [`mic`]).
#![forbid(unsafe_code)]

pub mod band_levels;
pub mod band_log;
pub mod check;
pub mod columns;
pub mod ir_wav;
pub mod math;
pub mod meta;
pub mod mic;
pub mod ops;
pub mod raw;
pub mod session;
pub mod smooth;
pub mod text;

pub use columns::{Columns, StoredTrace, frequencies, resample};
pub use ops::{OpError, average, capture_columns};
pub use session::SessionError;
pub use text::{ImportError, Imported, export_csv, import};

use ac2_proto::ctrl::ImportProblem;
use ac2_proto::model::{ImportFormat, ImportRole, IrImport};

/// Parses a file sent to `trace.import`: a WAV / RF64 impulse response ([`ir_wav`]) when
/// `format` says so or, for `auto`, when the file starts like one; text otherwise
/// ([`text::import`]). Impulse-response options with a text file, and an impulse response
/// as a target curve, are refused.
pub fn import_file(
    content: &[u8],
    format: ImportFormat,
    role: ImportRole,
    ir: Option<IrImport>,
) -> Result<Imported, ImportError> {
    let wav = match format {
        ImportFormat::IrWav => true,
        ImportFormat::Auto => ir_wav::is_wav(content),
        ImportFormat::Ac2Csv | ImportFormat::AnalyzerText => false,
    };
    if !wav {
        if ir.is_some() {
            return Err(text::fail(
                None,
                ImportProblem::BadOptions,
                "impulse-response options are for a WAV file; this one is text",
            ));
        }
        return import(content, format, role);
    }
    if role == ImportRole::Target {
        return Err(text::fail(
            None,
            ImportProblem::BadOptions,
            "an impulse response is a measured response, not a target curve: import it as a trace",
        ));
    }
    ir_wav::import(content, ir.unwrap_or_default())
}
