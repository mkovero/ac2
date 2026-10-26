//! Calibration: SPL cal, cal methods and basis, mics and mic curves, output/input setup.

use serde::{Deserialize, Serialize};

use super::DeviceId;
use crate::units::{Db, DbSpl, Dbfs, Dbu, Hz, MvPerPa, Ohms, Volts, WallNs};

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
        /// Mic sensitivity used, as the data sheet states it (open circuit).
        mic_sensitivity: MvPerPa,
        /// Where it came from.
        mic_sensitivity_from: SensitivitySource,
        /// The mic's output impedance against the preamp's input impedance, when stated:
        /// the voltage per pascal at the input is the open-circuit one divided down by
        /// them, and `SplCal::sensitivity` counts that. `None`: taken as unloaded, which is
        /// also what a stored calibration (store file, capture sidecar) without the field was.
        #[serde(default)]
        loading: Option<MicLoading>,
        /// Stated uncertainty of the sensitivity, ± dB.
        uncertainty: Db,
    },
}

/// The mic's source impedance and the preamp's input impedance it drives. A data-sheet
/// sensitivity is the open-circuit voltage; the preamp's input impedance and the mic's
/// output impedance form a divider, so the input sees `S · Z_in / (Z_in + Z_out)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicLoading {
    /// Mic output (nominal) impedance.
    pub mic_impedance: Ohms,
    /// Preamp (mic input) impedance.
    pub input_impedance: Ohms,
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
        /// The loading counted, if any.
        loading: Option<MicLoading>,
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
                loading,
                uncertainty,
                ..
            } => CalBasis::Electrical {
                connection: *connection,
                mic_sensitivity: *mic_sensitivity,
                data_sheet: matches!(mic_sensitivity_from, SensitivitySource::DataSheet { .. }),
                loading: *loading,
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

/// What an input carries, declared by the operator (decision V1): a backend cannot tell an
/// AES feed from an analogue one, so nothing is inferred. The kind fixes the dB reference
/// and which calibrations are offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputKind {
    /// A measurement microphone: dB SPL through its sensitivity calibration. The mic name
    /// and curve live here, so a line or digital input cannot carry a mic name at all.
    Mic {
        /// Mic name; `None` = not set.
        mic: Option<String>,
        /// The active curve.
        curve: CurveChoice,
    },
    /// Analogue line level: dBu through a line calibration, dBFS without one.
    Line,
    /// A digital feed (AES, ADAT, Dante…): dBFS; 0 dBFS is the source's full scale, so no
    /// calibration is ever offered.
    Digital,
}

impl InputKind {
    /// The mic name, on a `Mic`.
    pub fn mic(&self) -> Option<&str> {
        match self {
            Self::Mic { mic, .. } => mic.as_deref(),
            Self::Line | Self::Digital => None,
        }
    }

    /// The curve choice, on a `Mic`.
    pub fn curve(&self) -> Option<&CurveChoice> {
        match self {
            Self::Mic { curve, .. } => Some(curve),
            Self::Line | Self::Digital => None,
        }
    }

    /// The kind in words for a refusal: `microphone`, `line input`, `digital input`.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Mic { .. } => "microphone",
            Self::Line => "line input",
            Self::Digital => "digital input",
        }
    }
}

/// Which side of a stereo pair an input is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairSide {
    /// Left.
    Left,
    /// Right.
    Right,
}

/// Two inputs of one kind declared a stereo pair (decision V2): two mics, two line or two
/// digital inputs. A mic pair gets level bars and correlation but no loudness, since
/// BS.1770 loudness is defined on a programme feed, not on an acoustic pickup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pair {
    /// Zero-based device input channel of the left side.
    pub left: u16,
    /// Zero-based device input channel of the right side.
    pub right: u16,
    /// Operator's name; `None` = named by its inputs.
    pub name: Option<String>,
}

/// Input setup of one input channel: what is on it and, when paired, which side it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSetup {
    /// Zero-based device input channel.
    pub channel: u16,
    /// What is on it.
    pub kind: InputKind,
    /// Its side of the pair it is in, if any (the daemon sets it from the pair list).
    pub pair: Option<PairSide>,
}

impl InputSetup {
    /// The mic name, on a mic.
    pub fn mic(&self) -> Option<&str> {
        self.kind.mic()
    }

    /// The curve choice of a mic; `NotChosen` on a line or digital input (no curve can
    /// apply there).
    pub fn curve(&self) -> CurveChoice {
        self.kind.curve().cloned().unwrap_or(CurveChoice::NotChosen)
    }

    /// Makes the input a mic named `mic` (`None`: unnamed); another capsule's curves are
    /// its own, so a changed name leaves the curve unchosen.
    pub fn set_mic(&mut self, mic: Option<String>) {
        if !matches!(self.kind, InputKind::Mic { .. }) || self.kind.mic() != mic.as_deref() {
            self.kind = InputKind::Mic {
                mic,
                curve: CurveChoice::NotChosen,
            };
        }
    }

    /// Chooses `curve` on the input's mic (a line or digital input becomes an unnamed mic:
    /// a curve is a mic's).
    pub fn set_curve(&mut self, curve: CurveChoice) {
        self.kind = InputKind::Mic {
            mic: self.kind.mic().map(str::to_owned),
            curve,
        };
    }
}

/// The input setup as a whole: the rows and the pairs among them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSetups {
    /// One row per declared channel, sorted by channel.
    pub inputs: Vec<InputSetup>,
    /// Pairs, sorted by left channel.
    pub pairs: Vec<Pair>,
}

/// What an input of a device preview is declared to be (`session.preview`), so a client
/// can label the preview's rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    /// Zero-based device input channel.
    pub channel: u16,
    /// What is on it.
    pub kind: InputKind,
}

/// What a line calibration is tied to: a line input has no capsule to name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineCalKey {
    /// Capture device.
    pub device: DeviceId,
    /// Input channel.
    pub channel: u16,
}

/// A line calibration: the volts at 0 dBFS of a line input, from a steady tone whose RMS
/// voltage the operator measured while the meter read its level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineCal {
    /// Voltage at 0 dBFS: `volts / 10^(measured / 20)`.
    pub v_fs: Volts,
    /// Voltage measured, RMS.
    pub volts: Volts,
    /// Level read from the tone.
    pub measured: Dbfs,
    /// When (daemon clock).
    pub calibrated_at: WallNs,
}

/// Line calibration entry: one per device + input channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineCalEntry {
    /// Key.
    pub key: LineCalKey,
    /// The calibration.
    pub line: LineCal,
}

/// A calibration `cal.delete` addresses: either kind of entry of the store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalRef {
    /// A sensitivity calibration.
    Spl(CalKey),
    /// A line calibration.
    Line(LineCalKey),
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
    /// A line input with a line calibration: dBu, not dB SPL.
    Line {
        /// When it was taken.
        calibrated_at: WallNs,
        /// dBu of 0 dBFS.
        dbu_fs: Dbu,
    },
}
