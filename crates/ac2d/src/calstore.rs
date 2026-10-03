//! Calibration store (`docs/design/q7-calibration.md`): entries keyed by device + input
//! channel + mic name, mic-curve points, the input setup; the JSON file behind them; and
//! the matching rules that decide which calibration and curve a job on an input uses.
//!
//! The file is read once at start. One that cannot be read is never written: the store
//! comes up empty and read-only and every calibration command is refused with the reason,
//! so an operator's calibrations are never replaced by an empty set.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ac2_core::mic_curve::{Correction, MicCurve, MicCurveFileError};
use ac2_proto::model::{CalEntry, CalKey, CalStatus, DeviceId, InputSetup, MicCurveRef, SplCal};
use ac2_proto::{ErrorCode, ErrorDetail, MicCurveFileReason, ProtoError};
use serde::{Deserialize, Serialize};

use crate::util::{perr, perr_detail};

const FORMAT: &str = "ac2-calibrations";
const VERSION: u32 = 1;
/// Longest mic name, characters.
pub(crate) const MAX_MIC_NAME: usize = 64;
/// Normalisation frequency when no sensitivity calibration applies.
pub(crate) const DEFAULT_F_NORM: f64 = 1000.0;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileCurve {
    reference: MicCurveRef,
    /// (Hz, dB) pairs, ascending.
    points: Vec<[f64; 2]>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileEntry {
    key: CalKey,
    spl: Option<SplCal>,
    mic_curve: Option<FileCurve>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreFile {
    format: String,
    version: u32,
    entries: Vec<FileEntry>,
    inputs: Vec<InputSetup>,
}

/// What a job on one input uses.
#[derive(Debug, Clone)]
pub(crate) struct InputCal {
    pub(crate) status: CalStatus,
    /// dB SPL of 0 dBFS.
    pub(crate) sensitivity: Option<f64>,
    /// Mic curve normalised at the calibrator frequency, when one applies and is on.
    pub(crate) correction: Option<Arc<Correction>>,
    /// The entry whose sensitivity calibration is applied.
    pub(crate) spl_entry: Option<(CalKey, SplCal)>,
    /// Name of the applied mic curve.
    pub(crate) curve_name: Option<String>,
}

impl InputCal {
    pub(crate) fn none() -> Self {
        Self {
            status: CalStatus::Uncalibrated,
            sensitivity: None,
            correction: None,
            spl_entry: None,
            curve_name: None,
        }
    }
}

/// The daemon's calibration store.
#[derive(Debug)]
pub(crate) struct CalStore {
    path: Option<PathBuf>,
    /// Why the file could not be read; the store is then read-only.
    unreadable: Option<String>,
    curves: HashMap<CalKey, Arc<MicCurve>>,
}

/// Validates a mic name: 1 … 64 characters, no control characters, no surrounding space.
pub(crate) fn check_mic_name(m: &str) -> Result<(), ProtoError> {
    let inv = |msg: String| Err(perr(ErrorCode::Invalid, msg));
    if m.is_empty() {
        return inv("mic name is required".into());
    }
    if m.chars().count() > MAX_MIC_NAME {
        return inv(format!("mic name longer than {MAX_MIC_NAME} characters"));
    }
    if m.trim() != m || m.chars().any(char::is_control) {
        return inv(format!(
            "mic name {m:?} has surrounding spaces or control characters"
        ));
    }
    Ok(())
}

/// The protocol error for a refused mic-curve file.
pub(crate) fn curve_error(e: MicCurveFileError) -> ProtoError {
    let reason = match e {
        MicCurveFileError::TooFewPoints { .. } => MicCurveFileReason::TooFewPoints,
        MicCurveFileError::TooManyPoints { .. } => MicCurveFileReason::TooManyPoints,
        MicCurveFileError::BadNumber { .. } => MicCurveFileReason::BadNumber,
        MicCurveFileError::MissingGain { .. } => MicCurveFileReason::MissingGain,
        MicCurveFileError::NonPositiveFrequency { .. } => MicCurveFileReason::NonPositiveFrequency,
        MicCurveFileError::NonFinite { .. } => MicCurveFileReason::NonFinite,
        MicCurveFileError::GainOutOfRange { .. } => MicCurveFileReason::GainOutOfRange,
        MicCurveFileError::NotAscending { .. } => MicCurveFileReason::NotAscending,
    };
    perr_detail(
        ErrorCode::Invalid,
        format!("mic curve file refused: {e}"),
        ErrorDetail::MicCurveFile {
            line: e.line().map(|l| u32::try_from(l).unwrap_or(u32::MAX)),
            reason,
        },
    )
}

/// FNV-1a 64 of `b`, 16 lowercase hex digits.
pub(crate) fn content_hash(b: &[u8]) -> String {
    let h = b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| {
        (h ^ u64::from(x)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{h:016x}")
}

impl CalStore {
    /// A store without a file (tests, in-process daemons without a config directory).
    pub(crate) fn memory() -> Self {
        Self {
            path: None,
            unreadable: None,
            curves: HashMap::new(),
        }
    }

    /// Reads `path`. Missing → empty. Unreadable → empty and read-only, never written.
    /// Returns the store, its entries and the input setup.
    pub(crate) fn open(path: &Path) -> (Self, Vec<CalEntry>, Vec<InputSetup>) {
        let mut store = Self {
            path: Some(path.to_owned()),
            unreadable: None,
            curves: HashMap::new(),
        };
        let text = match std::fs::read(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return (store, Vec::new(), Vec::new());
            }
            Err(e) => {
                store.refuse(format!("cannot read it: {e}"));
                return (store, Vec::new(), Vec::new());
            }
        };
        match parse(&text) {
            Ok((entries, curves, inputs)) => {
                store.curves = curves;
                tracing::info!(
                    "calibration store {}: {} entries",
                    path.display(),
                    entries.len()
                );
                (store, entries, inputs)
            }
            Err(reason) => {
                store.refuse(reason);
                (store, Vec::new(), Vec::new())
            }
        }
    }

    fn refuse(&mut self, reason: String) {
        if let Some(p) = &self.path {
            tracing::error!(
                "calibration store {} is unreadable ({reason}); it will not be written — \
                 fix or move it away and restart ac2d",
                p.display()
            );
        }
        self.unreadable = Some(reason);
    }

    /// Ok unless the file could not be read at start.
    pub(crate) fn check(&self) -> Result<(), ProtoError> {
        match (&self.unreadable, &self.path) {
            (Some(reason), Some(path)) => Err(perr_detail(
                ErrorCode::Refused,
                format!(
                    "calibration store {} is unreadable ({reason}); it is never overwritten — \
                     fix or move it away and restart ac2d",
                    path.display()
                ),
                ErrorDetail::CalStore {
                    path: path.display().to_string(),
                    reason: reason.clone(),
                },
            )),
            _ => Ok(()),
        }
    }

    /// The points of the curve on `key`.
    pub(crate) fn curve(&self, key: &CalKey) -> Option<&Arc<MicCurve>> {
        self.curves.get(key)
    }

    /// Writes `entries` and `inputs` with `curves` (the full new curve set) atomically, then
    /// adopts `curves`. Nothing changes when the write fails.
    pub(crate) fn persist(
        &mut self,
        entries: &[CalEntry],
        inputs: &[InputSetup],
        curves: HashMap<CalKey, Arc<MicCurve>>,
    ) -> Result<(), ProtoError> {
        self.check()?;
        if let Some(path) = &self.path {
            let file = StoreFile {
                format: FORMAT.into(),
                version: VERSION,
                entries: entries
                    .iter()
                    .map(|e| FileEntry {
                        key: e.key.clone(),
                        spl: e.spl,
                        mic_curve: e.mic_curve.as_ref().and_then(|r| {
                            curves.get(&e.key).map(|c| FileCurve {
                                reference: r.clone(),
                                points: c
                                    .freqs()
                                    .iter()
                                    .zip(c.gains())
                                    .map(|(f, g)| [*f, *g])
                                    .collect(),
                            })
                        }),
                    })
                    .collect(),
                inputs: inputs.to_vec(),
            };
            let text = serde_json::to_vec_pretty(&file)
                .map_err(|e| perr(ErrorCode::Internal, format!("calibration store: {e}")))?;
            ac2_paths::write_private_atomic(path, &text).map_err(|e| {
                perr(
                    ErrorCode::Internal,
                    format!("cannot write calibration store {}: {e}", path.display()),
                )
            })?;
        }
        self.curves = curves;
        Ok(())
    }

    /// A copy of the curve set to modify before [`CalStore::persist`].
    pub(crate) fn curves(&self) -> HashMap<CalKey, Arc<MicCurve>> {
        self.curves.clone()
    }
}

type Parsed = (
    Vec<CalEntry>,
    HashMap<CalKey, Arc<MicCurve>>,
    Vec<InputSetup>,
);

fn parse(text: &[u8]) -> Result<Parsed, String> {
    let f: StoreFile = serde_json::from_slice(text).map_err(|e| e.to_string())?;
    if f.format != FORMAT || f.version != VERSION {
        return Err(format!(
            "format {:?} version {}, expected {FORMAT:?} version {VERSION}",
            f.format, f.version
        ));
    }
    let mut entries: Vec<CalEntry> = Vec::new();
    let mut curves = HashMap::new();
    for e in f.entries {
        if entries.iter().any(|x| x.key == e.key) {
            return Err(format!("duplicate entry {:?}", e.key));
        }
        check_mic_name(&e.key.mic).map_err(|p| p.msg)?;
        let reference = match e.mic_curve {
            None => None,
            Some(c) => {
                let pts: Vec<(f64, f64)> = c.points.iter().map(|p| (p[0], p[1])).collect();
                let curve = MicCurve::from_points(&pts)
                    .map_err(|err| format!("mic curve of {:?}: {err}", e.key))?;
                curves.insert(e.key.clone(), Arc::new(curve));
                Some(c.reference)
            }
        };
        if e.spl.is_none() && reference.is_none() {
            return Err(format!("entry {:?} holds nothing", e.key));
        }
        entries.push(CalEntry {
            key: e.key,
            spl: e.spl,
            mic_curve: reference,
        });
    }
    let mut inputs = f.inputs;
    inputs.sort_by_key(|i| i.channel);
    if inputs.windows(2).any(|w| w[0].channel == w[1].channel) {
        return Err("duplicate input channel".into());
    }
    for i in &inputs {
        if let Some(m) = &i.mic {
            check_mic_name(m).map_err(|p| p.msg)?;
        }
    }
    Ok((entries, curves, inputs))
}

/// The input setup row of `channel` (defaults: no mic name, curve on).
pub(crate) fn input_setup(inputs: &[InputSetup], channel: u16) -> InputSetup {
    inputs
        .iter()
        .find(|i| i.channel == channel)
        .cloned()
        .unwrap_or(InputSetup {
            channel,
            mic: None,
            mic_curve: true,
        })
}

/// The entry whose mic curve applies to mic `mic` (Q7 §3): a curve is a property of the
/// capsule, so it follows the mic name — the entry on `at` (device, input) when it holds
/// one, else the newest curve imported for that mic anywhere.
pub(crate) fn curve_entry<'a>(
    entries: &'a [CalEntry],
    mic: &str,
    at: Option<(&DeviceId, u16)>,
) -> Option<&'a CalEntry> {
    let with_curve = || {
        entries
            .iter()
            .filter(move |e| e.key.mic == mic && e.mic_curve.is_some())
    };
    at.and_then(|(d, c)| with_curve().find(|e| e.key.device == *d && e.key.channel == c))
        .or_else(|| with_curve().max_by_key(|e| e.mic_curve.as_ref().map(|c| c.imported_at)))
}

/// Which calibration and mic curve a job on `channel` of `device` uses (Q7 §3).
pub(crate) fn resolve(
    entries: &[CalEntry],
    inputs: &[InputSetup],
    store: &CalStore,
    device: &DeviceId,
    channel: u16,
) -> InputCal {
    let setup = input_setup(inputs, channel);
    let mic = setup.mic.as_deref();
    let here = |e: &&CalEntry| e.key.device == *device && e.key.channel == channel;
    let with_spl = || entries.iter().filter(|e| e.spl.is_some());
    let newest = |it: Vec<&CalEntry>| {
        it.into_iter()
            .max_by_key(|e| e.spl.map(|s| s.calibrated_at))
            .and_then(|e| e.spl.map(|s| (e.key.clone(), s)))
    };
    let exact = mic.and_then(|m| {
        with_spl()
            .find(|e| here(e) && e.key.mic == m)
            .and_then(|e| e.spl.map(|s| (e.key.clone(), s)))
    });
    let (status, spl_entry) = if let Some((k, s)) = exact {
        (
            CalStatus::Verified {
                calibrated_at: s.calibrated_at,
            },
            Some((k, s)),
        )
    } else if let Some((k, s)) = newest(with_spl().filter(here).collect())
        .or_else(|| mic.and_then(|m| newest(with_spl().filter(|e| e.key.mic == m).collect())))
    {
        (
            CalStatus::OtherMicOrInput {
                calibrated_at: s.calibrated_at,
            },
            Some((k, s)),
        )
    } else {
        (CalStatus::Uncalibrated, None)
    };
    let spl = spl_entry.as_ref().map(|(_, s)| *s);

    let curve = match (mic, setup.mic_curve) {
        (Some(m), true) => curve_entry(entries, m, Some((device, channel))).and_then(|e| {
            let name = e.mic_curve.as_ref().map(|r| r.name.clone())?;
            store.curve(&e.key).map(|c| (name, c))
        }),
        _ => None,
    };
    let f_norm = spl.map_or(DEFAULT_F_NORM, |s| s.calibrator_freq.0);
    InputCal {
        status,
        sensitivity: spl.map(|s| s.sensitivity.0),
        correction: curve.as_ref().map(|(_, c)| Arc::new(c.normalised(f_norm))),
        spl_entry,
        curve_name: curve.map(|(n, _)| n),
    }
}

#[cfg(test)]
mod tests;
