//! Arguments of the commands that bring a measured value or file in from outside: an
//! imported trace file and an electrically measured input voltage.

use serde::{Deserialize, Serialize};

use crate::model::{
    ElectricalConnection, ImportFormat, ImportRole, IrImport, MicLoading, TraceOwner,
};
use crate::units::{Blob, Db, Hz, MvPerPa, TransferId, Volts};

/// Arguments of `trace.import`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceImportArgs {
    /// The complete upload; its `file`'s last component names a trace its file does not.
    pub transfer: TransferId,
    /// Format.
    pub format: ImportFormat,
    /// Measured trace or target curve.
    pub role: ImportRole,
    /// How to read an impulse-response WAV file; `None` takes [`IrImport::default`].
    /// Refused (`invalid`) with a text file.
    pub ir: Option<IrImport>,
    /// The measurement it is listed under, or the imported group: a target loaded for
    /// the system being tuned belongs with that system's measurement. Refused for a
    /// math trace or a measurement that does not exist.
    pub owner: TraceOwner,
}

/// Arguments of `cal.curve_import`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalCurveImportArgs {
    /// Mic name.
    pub mic: String,
    /// Label; `None` = from the file.
    pub label: Option<String>,
    /// Original file name.
    pub file_name: String,
    /// File content (`.frd`, `.txt`, CSV); the daemon parses and validates it.
    pub content: Blob,
    /// Input channel whose mic name is set.
    pub channel: Option<u16>,
}

/// Arguments of `cal.spl_electrical`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalSplElectricalArgs {
    /// Input channel.
    pub channel: u16,
    /// Mic name.
    pub mic: String,
    /// Where the voltage was measured.
    pub connection: ElectricalConnection,
    /// Voltage measured, RMS.
    pub volts: Volts,
    /// Frequency of the tone.
    pub freq: Hz,
    /// Mic sensitivity; `None` = the data sheet's.
    pub mic_sensitivity: Option<MvPerPa>,
    /// Mic output and preamp input impedance: the sensitivity is divided down by them
    /// (`cal::loaded_sensitivity`). `None` = unloaded.
    pub loading: Option<MicLoading>,
    /// Stated uncertainty, ± dB; `None` = 1 dB.
    pub uncertainty: Option<Db>,
    /// Replace an acoustic calibration of this input and mic (refused otherwise: a
    /// calibrator reading is the better one).
    pub replace_acoustic: bool,
}
