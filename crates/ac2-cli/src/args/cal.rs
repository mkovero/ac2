//! `ac2 cal …`: calibration.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use super::CurveArg;
use crate::units::{Channel, Freq, Gain, MicSensitivityArg, SplLevel, VoltsArg};

/// `cal …`.
#[derive(Debug, Subcommand)]
pub enum CalCmd {
    /// Calibrate an input against an acoustic calibrator.
    Spl(CalSpl),
    /// Calibrate an input without a calibrator: a voltage measured at the input with a
    /// meter (DMM, Analog Discovery) while ac2 reads its level, and the mic's sensitivity.
    ///
    /// In-line (default): the mic stays connected and powered and hears a steady tone
    /// (1 kHz); measure AC volts between XLR pins 2 and 3 with a breakout. Phantom power is
    /// +48 V on pins 2 and 3 against pin 1: measure pins 2–3 only, never to pin 1, never
    /// short pins. Injected: a generator in place of the mic — switch phantom power OFF on
    /// that input first (48 V can damage the generator; ac2 cannot switch it) and back on
    /// for the mic afterwards. Either way keep the gain you will measure with.
    Electrical(CalElectrical),
    /// The mic library: import, rename and delete a mic's curves.
    #[command(subcommand)]
    Curve(CalCurveCmd),
    /// Choose the mic curve an input applies: a label of its mic's curves, or `off`.
    Use {
        /// Input channel.
        input: Channel,
        /// Curve label (e.g. 90°), or `off`.
        curve: CurveArg,
    },
    /// List sensitivity calibrations, the mic library and what each input uses.
    List,
    /// Delete a sensitivity calibration.
    Rm(CalRm),
}

/// `cal curve …`.
#[derive(Debug, Subcommand)]
pub enum CalCurveCmd {
    /// Import a mic's magnitude curve (.frd / .txt / CSV) into the mic library. The label
    /// defaults to the angle the file names (`90°`), else its file name. With `--input`, the
    /// mic becomes that input's mic, and its only curve its active one.
    Import(CalCurveImport),
    /// Rename a curve (inputs that use it follow).
    Rename {
        /// Mic name.
        #[arg(long)]
        mic: String,
        /// Current label.
        label: String,
        /// New label.
        new_label: String,
    },
    /// Delete a curve from the mic library.
    Rm {
        /// Mic name.
        #[arg(long)]
        mic: String,
        /// Label.
        label: String,
    },
}

/// `cal curve import`.
#[derive(Debug, Args)]
pub struct CalCurveImport {
    /// Curve file: frequency and dB per line (further columns ignored).
    pub file: PathBuf,
    /// Mic name (default: the mic on `--input`).
    #[arg(long, required_unless_present = "input")]
    pub mic: Option<String>,
    /// Label among the mic's curves, e.g. `90°` (default: from the file).
    #[arg(long)]
    pub label: Option<String>,
    /// Input the mic is on: sets its mic name.
    #[arg(long)]
    pub input: Option<Channel>,
}

/// `cal rm`.
#[derive(Debug, Args)]
pub struct CalRm {
    /// Input channel of the calibration.
    #[arg(long)]
    pub input: Channel,
    /// Mic name of the calibration (default: the input's mic name in the session's input
    /// setup).
    #[arg(long)]
    pub mic: Option<String>,
    /// Capture device of the calibration (default: the open session's; without a session,
    /// the one device holding a calibration for this input and mic).
    #[arg(long, value_name = "ID")]
    pub device: Option<String>,
}

/// Where the voltage of an electrical calibration is measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ElectricalMethodArg {
    /// Pins 2–3 with the mic connected and powered, a tone at the mic.
    Inline,
    /// A generator in place of the mic, phantom power off.
    Injected,
}

/// `cal electrical`.
#[derive(Debug, Args)]
pub struct CalElectrical {
    /// Input channel.
    #[arg(long)]
    pub input: Channel,
    /// Voltage measured at the input, RMS, e.g. `15.03mv`.
    #[arg(long, value_name = "VOLTS")]
    pub volts: VoltsArg,
    /// Frequency of the tone (mic sensitivities are stated at 1 kHz).
    #[arg(long, default_value = "1khz")]
    pub freq: Freq,
    /// Mic sensitivity, e.g. `15.0mv/pa` or `-36.5dbv/pa` (default: the data-sheet value the
    /// mic's curve files state, when they state one).
    #[arg(long, value_name = "MV/PA", allow_hyphen_values = true)]
    pub sensitivity: Option<MicSensitivityArg>,
    /// Where the voltage is measured.
    #[arg(long, value_enum, default_value = "inline")]
    pub method: ElectricalMethodArg,
    /// Stated uncertainty, e.g. `0.5db` (default ±1 dB: the data-sheet tolerance).
    #[arg(long, value_name = "DB")]
    pub uncertainty: Option<Gain>,
    /// Mic name the calibration is bound to (default: the input's mic name in the
    /// session's input setup); it also becomes the input's mic name.
    #[arg(long)]
    pub mic: Option<String>,
    /// Replace an acoustic calibration of this input and mic (a calibrator reading is the
    /// better one; refused otherwise).
    #[arg(long)]
    pub replace_acoustic: bool,
}

/// `cal spl`.
#[derive(Debug, Args)]
pub struct CalSpl {
    /// Input channel with the calibrator on its mic.
    #[arg(long)]
    pub input: Channel,
    /// Calibrator level, e.g. `94db`.
    #[arg(long = "ref", value_name = "SPL")]
    pub reference: SplLevel,
    /// Calibrator frequency.
    #[arg(long, default_value = "1khz")]
    pub freq: Freq,
    /// Mic name the calibration is bound to (default: the input's mic name in the
    /// session's input setup); it also becomes the input's mic name.
    #[arg(long)]
    pub mic: Option<String>,
}
