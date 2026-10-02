//! Stored traces outside the live DSP path: the column data a trace holds, capture from a
//! published frame, averaging and A−B math, the text formats traces are imported from and
//! exported to, and the session directory format.
//!
//! The daemon owns the trace store and calls into this crate; the client's fake daemon uses
//! the same code so tests against it see real numbers. Display edits (offset, polarity,
//! nudge) are never baked into stored columns: columns stay as measured.
#![forbid(unsafe_code)]

pub mod columns;
pub mod meta;
pub mod ops;
pub mod session;
pub mod text;

pub use columns::{Columns, StoredTrace, frequencies, resample};
pub use ops::{OpError, average, capture_columns, math};
pub use session::{SessionError, default_session_dir};
pub use text::{ImportError, Imported, export_csv, import};
