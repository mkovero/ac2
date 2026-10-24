//! Electrical and line calibration math (`docs/design/q7-calibration.md` §11,
//! `programme-meter.md` §1.1): volts at full scale, dBu, mic loading, data-sheet
//! sensitivities.

use super::mic;
use crate::model::{CalMethod, Mic, MicLoading, SensitivitySource, SplCal};
use crate::units::{Db, Dbu, Hz, MvPerPa};

/// Reference sound pressure of dB SPL, Pa.
pub const P_REF_PA: f64 = 20e-6;
/// Frequency mic sensitivities are specified at (IEC 61094, every data sheet).
pub const MIC_SENSITIVITY_FREQ: Hz = Hz(1000.0);
/// Stated uncertainty of an electrical calibration when the operator states none, ± dB:
/// the data-sheet sensitivity's tolerance dominates (§11).
pub const DEFAULT_ELECTRICAL_UNCERTAINTY: Db = Db(1.0);
/// Highest level read for an electrical calibration, dBFS: closer to full scale, a
/// converter's or preamp's onset of compression (and the next gain change) is too near.
pub const ELECTRICAL_MAX_DBFS: f64 = -3.0;
/// Lowest level read, dBFS: below it the input's noise adds to the tone (−70 dBFS with a
/// typical −110 dBFS noise floor is still 40 dB clear, an error below 0.001 dB).
pub const ELECTRICAL_MIN_DBFS: f64 = -70.0;
/// Stated uncertainties accepted, ± dB.
pub const ELECTRICAL_UNCERTAINTY_RANGE: (f64, f64) = (0.05, 6.0);

/// Volts at 0 dBFS from `volts` (RMS) read as `level_dbfs`.
pub fn full_scale_volts(volts: f64, level_dbfs: f64) -> f64 {
    volts / 10f64.powf(level_dbfs / 20.0)
}

/// dB SPL of 0 dBFS for an input whose full scale is `full_scale_volts` with a mic of
/// `mv_per_pa`: 0 dBFS is `V_FS / S` pascal.
pub fn electrical_sensitivity_db(full_scale_volts: f64, mv_per_pa: f64) -> f64 {
    20.0 * (full_scale_volts / (mv_per_pa / 1000.0 * P_REF_PA)).log10()
}

// ---------------------------------------------------------------------------------------
// Line calibration (`docs/design/programme-meter.md` §1.1)

/// 0 dBu in volts RMS: 1 mW into 600 Ω, `sqrt(0.001 · 600)`.
pub const DBU_REF_VOLTS: f64 = 0.774_596_669_241_483_4;
/// Lowest and highest line full scale accepted, dBu: below +4 dBu a line input could not
/// take a nominal signal; above +30 dBu no interface exists (the volts would be a unit slip).
pub const LINE_DBU_FS_RANGE: (f64, f64) = (4.0, 30.0);

/// dBu of 0 dBFS for an input whose full scale is `full_scale_volts`.
pub fn dbu_fs(full_scale_volts: f64) -> Dbu {
    Dbu(20.0 * (full_scale_volts / DBU_REF_VOLTS).log10())
}

/// The level of `volts` RMS on a line, dBu.
pub fn volts_to_dbu(volts: f64) -> Dbu {
    Dbu(20.0 * (volts / DBU_REF_VOLTS).log10())
}

/// An impedance of a [`MicLoading`] that cannot form a voltage divider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadingError {
    /// The mic's output impedance is not a finite value above 0 Ω.
    MicImpedance(f64),
    /// The preamp's input impedance is not a finite value above 0 Ω.
    InputImpedance(f64),
}

impl std::fmt::Display for LoadingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MicImpedance(z) => write!(f, "mic output impedance {z} Ω is not above 0 Ω"),
            Self::InputImpedance(z) => {
                write!(f, "preamp input impedance {z} Ω is not above 0 Ω")
            }
        }
    }
}

impl std::error::Error for LoadingError {}

/// Level change of the mic's voltage when it drives the preamp, dB (≤ 0): the mic's output
/// impedance and the preamp's input impedance divide the open-circuit voltage a data sheet
/// states, `20·lg(Z_in / (Z_in + Z_out))`. Magnitudes only: both are near-resistive at
/// 1 kHz, where the sensitivity is specified.
pub fn loading_db(l: &MicLoading) -> Result<Db, LoadingError> {
    let ok = |z: f64| z.is_finite() && z > 0.0;
    if !ok(l.mic_impedance.0) {
        return Err(LoadingError::MicImpedance(l.mic_impedance.0));
    }
    if !ok(l.input_impedance.0) {
        return Err(LoadingError::InputImpedance(l.input_impedance.0));
    }
    let zin = l.input_impedance.0;
    Ok(Db(20.0 * (zin / (zin + l.mic_impedance.0)).log10()))
}

/// The sensitivity the preamp's input sees, mV/Pa: the open-circuit `mv_per_pa` divided
/// down by `loading` (`S · Z_in / (Z_in + Z_out)`), or `mv_per_pa` itself without one.
pub fn loaded_sensitivity(
    mv_per_pa: MvPerPa,
    loading: Option<&MicLoading>,
) -> Result<MvPerPa, LoadingError> {
    match loading {
        None => Ok(mv_per_pa),
        Some(l) => {
            loading_db(l)?;
            let zin = l.input_impedance.0;
            Ok(MvPerPa(mv_per_pa.0 * zin / (zin + l.mic_impedance.0)))
        }
    }
}

/// The frequency a calibration normalises the mic curve at: the calibrator's (the tone was
/// read uncorrected through the capsule, so 0 dB there counts nothing twice), or 1 kHz for an
/// electrical one (the data-sheet sensitivity is the capsule's at 1 kHz).
pub fn f_norm(spl: &SplCal) -> Hz {
    match spl.method {
        CalMethod::Acoustic { .. } => spl.freq,
        CalMethod::Electrical { .. } => MIC_SENSITIVITY_FREQ,
    }
}

/// The data-sheet sensitivity a mic's curve files state.
#[derive(Debug, Clone, PartialEq)]
pub enum DataSheet {
    /// The mic is not in the library.
    NoMic,
    /// Its files state none.
    NoneStated,
    /// One value (several files may state it).
    One(MvPerPa, SensitivitySource),
    /// The files disagree: the operator says which.
    Differ(Vec<f64>),
}

/// The sensitivity `mic`'s curve files state.
pub fn data_sheet(mics: &[Mic], mic_name: &str) -> DataSheet {
    let Some(m) = mic(mics, mic_name) else {
        return DataSheet::NoMic;
    };
    let mut values: Vec<f64> = Vec::new();
    for v in m.curves.iter().filter_map(|c| c.stated_sensitivity) {
        if !values.contains(&v) {
            values.push(v);
        }
    }
    match values.as_slice() {
        [] => DataSheet::NoneStated,
        [v] => {
            let c = m
                .curves
                .iter()
                .find(|c| c.stated_sensitivity == Some(*v))
                .unwrap_or(&m.curves[0]);
            DataSheet::One(
                MvPerPa(*v),
                SensitivitySource::DataSheet {
                    label: c.label.clone(),
                    file_name: c.file_name.clone(),
                },
            )
        }
        _ => DataSheet::Differ(values),
    }
}
