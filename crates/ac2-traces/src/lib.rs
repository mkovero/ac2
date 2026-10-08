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
pub mod columns;
pub mod math;
pub mod meta;
pub mod mic;
pub mod ops;
pub mod raw;
pub mod session;
pub mod smooth;
pub mod spl_log;
pub mod text;

pub use columns::{Columns, StoredTrace, frequencies, resample};
pub use ops::{OpError, average, capture_columns};
pub use session::SessionError;
pub use text::{ImportError, Imported, export_csv, import};
