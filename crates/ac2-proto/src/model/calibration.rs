//! Calibration: SPL cal, cal methods and basis, mics and mic curves, output/input setup.

use serde::{Deserialize, Serialize};

use super::DeviceId;
use crate::units::{Db, DbSpl, Dbfs, Hz, MvPerPa, Volts, WallNs};

/// What a calibration is tied to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalKey {
    /// Capture device.
    pub device: DeviceId,
    /// Input channel.
    pub channel: u16,
    /// Mic name.
    pub mic: String,
}

/// A sensitivity calibration: what 0 dBFS on the input is in dB SPL, and how that was
/// found (`docs/design/q7-calibration.md` §2, §11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplCal {
    /// dB SPL = dBFS + sensitivity.
    pub sensitivity: Db,
    /// How it was measured.
    pub method: CalMethod,
    /// Frequency of the tone read.
    pub freq: Hz,
    /// Broadband level read from the tone, uncorrected.
    pub measured: Dbfs,
    /// When (daemon clock).
    pub calibrated_at: WallNs,
}

/// How a sensitivity calibration was measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalMethod {
    /// An acoustic calibrator on the mic: the whole chain, capsule included. The mic curve
    /// is normalised to 0 dB at the calibrator frequency.
    Acoustic {
        /// Calibrator level.
        calibrator_level: DbSpl,
    },
    /// A voltage measured at the input while ac2 read its level, with the mic's
    /// sensitivity from the operator or the data sheet. The mic curve is normalised to 0 dB
    /// at 1 kHz, where mic sensitivities are specified.
    Electrical {
        /// Where the voltage was measured.
        connection: ElectricalConnection,
        /// Voltage measured, RMS.
        volts: Volts,
        /// Voltage at 0 dBFS: `volts / 10^(measured / 20)`.
        full_scale: Volts,
        /// Mic sensitivity used.
        mic_sensitivity: MvPerPa,
        /// Where it came from.
        mic_sensitivity_from: SensitivitySource,
        /// Stated uncertainty of the sensitivity, ± dB.
        uncertainty: Db,
    },
}

/// Where the voltage of an electrical calibration was measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElectricalConnection {
    /// Across XLR pins 2–3 with the mic connected and powered and a steady tone at the
    /// mic: the mic's own source impedance loads the preamp as in use.
    InLine,
    /// A generator in place of the mic (phantom power off).
    Injected,
}

/// Where the mic sensitivity of an electrical calibration came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SensitivitySource {
    /// Typed by the operator.
    Typed,
    /// The stated sensitivity in the header of one of the mic's curve files.
    DataSheet {
        /// The curve's label.
        label: String,
        /// Its file name.
        file_name: String,
    },
}

/// What a readout's calibration rests on, as frames carry it: enough to word it
/// (`cal 94 dB`, `electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalBasis {
    /// An acoustic calibrator.
    Acoustic {
        /// Calibrator level.
        calibrator_level: DbSpl,
    },
    /// A measured voltage and the mic's sensitivity.
    Electrical {
        /// Where the voltage was measured.
        connection: ElectricalConnection,
        /// Mic sensitivity used.
        mic_sensitivity: MvPerPa,
        /// It is the data sheet's (else typed).
        data_sheet: bool,
        /// Stated uncertainty, ± dB.
        uncertainty: Db,
    },
}

impl CalMethod {
    /// What frames carry of it.
    pub fn basis(&self) -> CalBasis {
        match self {
            Self::Acoustic { calibrator_level } => CalBasis::Acoustic {
                calibrator_level: *calibrator_level,
            },
            Self::Electrical {
                connection,
                mic_sensitivity,
                mic_sensitivity_from,
                uncertainty,
                ..
            } => CalBasis::Electrical {
                connection: *connection,
                mic_sensitivity: *mic_sensitivity,
                data_sheet: matches!(mic_sensitivity_from, SensitivitySource::DataSheet { .. }),
                uncertainty: *uncertainty,
            },
        }
    }
}

/// Provenance of one imported mic curve (the points stay in the daemon's store).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicCurveRef {
    /// Short name among the mic's curves, e.g. `0°`, `90°` (unique per mic).
    pub label: String,
    /// File name as imported.
    pub file_name: String,
    /// FNV-1a 64 of the file bytes, 16 lowercase hex digits.
    pub content_hash: String,
    /// Points parsed.
    pub points: u32,
    /// Lowest point frequency.
    pub f_lo: Hz,
    /// Highest point frequency.
    pub f_hi: Hz,
    /// When (daemon clock).
    pub imported_at: WallNs,
    /// Sensitivity the file's header states, mV/Pa. Used only when the operator takes it
    /// for an electrical calibration (`cal.spl_electrical`); an acoustic calibration
    /// measures the whole chain instead.
    pub stated_sensitivity: Option<f64>,
}

/// A mic in the calibration store's mic library: its curves (one per incidence angle, or
/// whatever the operator keeps). Curves follow the mic name across inputs and devices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mic {
    /// Mic name, as on the input setup.
    pub name: String,
    /// Curves in import order; never empty (a mic without curves is not stored).
    pub curves: Vec<MicCurveRef>,
}

/// One curve of the mic library.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicCurveId {
    /// Mic name.
    pub mic: String,
    /// Curve label.
    pub label: String,
}

/// Sensitivity calibration entry: one per device + input channel + mic name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalEntry {
    /// Key.
    pub key: CalKey,
    /// The calibration.
    pub spl: SplCal,
}

/// Which of its mic's curves an input applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CurveChoice {
    /// Not chosen yet: no curve applies, and the operator is asked to choose. The daemon
    /// chooses the only curve of a mic that has exactly one.
    NotChosen,
    /// Explicitly no curve.
    Off,
    /// The mic's curve with this label.
    Curve {
        /// Label.
        label: String,
    },
}

/// An operator's name for one output channel of the rig (`Main L`, `Sub`), kept by the
/// daemon with the rig's settings (not in sessions: the wiring outlives a session).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSetup {
    /// Zero-based device output channel.
    pub channel: u16,
    /// The name; `None` clears it (the device's own channel name shows again).
    pub label: Option<String>,
}

/// Longest output label the daemon accepts, characters.
pub const MAX_OUTPUT_LABEL: usize = 32;

/// Checks an output label: 1 … [`MAX_OUTPUT_LABEL`] characters, no control characters, no
/// surrounding space.
pub fn check_output_label(l: &str) -> Result<(), String> {
    if l.is_empty() {
        return Err("an output label must not be empty (clear it instead)".into());
    }
    if l.chars().count() > MAX_OUTPUT_LABEL {
        return Err(format!(
            "an output label is at most {MAX_OUTPUT_LABEL} characters"
        ));
    }
    if l.chars().any(char::is_control) {
        return Err("an output label must not contain control characters".into());
    }
    if l.trim() != l {
        return Err("an output label must not start or end with a space".into());
    }
    Ok(())
}

/// Input setup of one input channel (decision K8): which mic is on it and which of the
/// mic's curves it applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSetup {
    /// Zero-based device input channel.
    pub channel: u16,
    /// Mic name; `None` = not set.
    pub mic: Option<String>,
    /// The active curve.
    pub curve: CurveChoice,
}

/// Calibration state of a calibrated readout (decisions 7a/7b). The age is the frame's
/// `capture_wall_ns − calibrated_at`, both on the daemon clock.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalStatus {
    /// No sensitivity calibration applies: dBFS.
    Uncalibrated,
    /// The calibration of this device + input + mic.
    Verified {
        /// When it was taken.
        calibrated_at: WallNs,
        /// What it rests on.
        basis: CalBasis,
    },
    /// A calibration of another mic on this input, or of this mic on another input or
    /// device, is applied.
    OtherMicOrInput {
        /// When it was taken.
        calibrated_at: WallNs,
        /// What it rests on.
        basis: CalBasis,
    },
}
