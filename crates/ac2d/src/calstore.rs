//! Calibration store (`docs/design/q7-calibration.md`): sensitivity calibrations keyed by
//! device + input channel + mic name, the mic library (named curves per mic, with their
//! points), the input setup; the JSON file behind them; and the matching that decides what a
//! job on an input uses.
//!
//! The file is read once at start. One that cannot be read is never written: the store
//! comes up empty and read-only and every calibration command is refused with the reason,
//! so an operator's calibrations are never replaced by an empty set. A store of another
//! format version is set aside (renamed, never deleted) and the daemon starts with an empty
//! store: there is no migration.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ac2_core::mic_curve::{Correction, MicCurve, MicCurveFileError};
use ac2_proto::cal::{self, InputUse};
use ac2_proto::model::{
    CalEntry, CalStatus, DeviceId, InputSetup, Mic, MicCurveId, MicCurveRef, State,
};
use ac2_proto::{ErrorCode, ErrorDetail, MicCurveFileReason, ProtoError};
use serde::{Deserialize, Serialize};

use crate::util::{perr, perr_detail};

const FORMAT: &str = "ac2-calibrations";
const VERSION: u32 = 2;
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
struct FileMic {
    name: String,
    curves: Vec<FileCurve>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreFile {
    format: String,
    version: u32,
    sensitivities: Vec<CalEntry>,
    mics: Vec<FileMic>,
    inputs: Vec<InputSetup>,
}

/// Just enough of any store file to tell its version.
#[derive(Debug, Deserialize)]
struct Header {
    format: String,
    version: u32,
}

/// What the store holds, as mirrored.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Contents {
    pub(crate) calibrations: Vec<CalEntry>,
    pub(crate) mics: Vec<Mic>,
    pub(crate) inputs: Vec<InputSetup>,
}

/// The curves' points, by mic and label.
pub(crate) type Curves = HashMap<MicCurveId, Arc<MicCurve>>;

/// What a job on one input uses.
#[derive(Debug, Clone)]
pub(crate) struct InputCal {
    pub(crate) status: CalStatus,
    /// dB SPL of 0 dBFS.
    pub(crate) sensitivity: Option<f64>,
    /// Mic curve normalised at the calibrator frequency, when one is chosen and stored.
    pub(crate) correction: Option<Arc<Correction>>,
    /// The entry whose sensitivity calibration is applied.
    pub(crate) spl_entry: Option<CalEntry>,
    /// The applied mic curve.
    pub(crate) curve: Option<MicCurveRef>,
}

impl InputCal {
    pub(crate) fn none() -> Self {
        Self {
            status: CalStatus::Uncalibrated,
            sensitivity: None,
            correction: None,
            spl_entry: None,
            curve: None,
        }
    }
}

/// The daemon's calibration store.
#[derive(Debug)]
pub(crate) struct CalStore {
    path: Option<PathBuf>,
    /// Why the file could not be read; the store is then read-only.
    unreadable: Option<String>,
    curves: Curves,
}

/// Validates a mic name: 1 … 64 characters, no control characters, no surrounding space.
pub(crate) fn check_mic_name(m: &str) -> Result<(), ProtoError> {
    cal::check_mic_name(m).map_err(|e| perr(ErrorCode::Invalid, e))
}

/// Validates a curve label.
pub(crate) fn check_label(l: &str) -> Result<(), ProtoError> {
    cal::check_label(l).map_err(|e| perr(ErrorCode::Invalid, e))
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

/// Renames `p` to `<p>.<tag>` (the time appended if that exists). Returns where it went.
fn set_aside(p: &Path, tag: &str) -> Option<PathBuf> {
    let name = p.file_name()?.to_string_lossy().into_owned();
    let mut to = p.with_file_name(format!("{name}.{tag}"));
    if to.exists() {
        to = p.with_file_name(format!(
            "{name}.{tag}-{}",
            crate::util::wall_ns() / 1_000_000_000
        ));
    }
    std::fs::rename(p, &to).ok().map(|()| to)
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

    /// Reads `path`. Missing → empty. Another format version → set aside, empty.
    /// Unreadable → empty and read-only, never written.
    pub(crate) fn open(path: &Path) -> (Self, Contents) {
        let mut store = Self {
            path: Some(path.to_owned()),
            unreadable: None,
            curves: HashMap::new(),
        };
        let text = match std::fs::read(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return (store, Contents::default());
            }
            Err(e) => {
                store.refuse(format!("cannot read it: {e}"));
                return (store, Contents::default());
            }
        };
        if let Ok(h) = serde_json::from_slice::<Header>(&text)
            && h.format == FORMAT
            && h.version != VERSION
        {
            match set_aside(path, &format!("v{}", h.version)) {
                Some(to) => {
                    tracing::warn!(
                        "calibration store {} is format version {} (this ac2d reads {VERSION}); \
                         set aside as {} and starting with an empty store — calibrate again \
                         (`ac2 cal spl`) and import the mic curves again (`ac2 cal curve import \
                         FILE --mic NAME`); the old file shows the mic names and file names",
                        path.display(),
                        h.version,
                        to.display()
                    );
                    return (store, Contents::default());
                }
                None => {
                    store.refuse(format!(
                        "format version {} (this ac2d reads {VERSION}) and it cannot be renamed \
                         out of the way",
                        h.version
                    ));
                    return (store, Contents::default());
                }
            }
        }
        match parse(&text) {
            Ok((contents, curves)) => {
                store.curves = curves;
                tracing::info!(
                    "calibration store {}: {} sensitivity calibrations, {} mics",
                    path.display(),
                    contents.calibrations.len(),
                    contents.mics.len()
                );
                (store, contents)
            }
            Err(reason) => {
                store.refuse(reason);
                (store, Contents::default())
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

    /// The points of a curve.
    pub(crate) fn curve(&self, mic: &str, label: &str) -> Option<&Arc<MicCurve>> {
        self.curves.get(&MicCurveId {
            mic: mic.to_owned(),
            label: label.to_owned(),
        })
    }

    /// Writes `c` with `curves` (the full new curve set) atomically, then adopts `curves`.
    /// Nothing changes when the write fails.
    pub(crate) fn persist(&mut self, c: &Contents, curves: Curves) -> Result<(), ProtoError> {
        self.check()?;
        if let Some(path) = &self.path {
            let file = StoreFile {
                format: FORMAT.into(),
                version: VERSION,
                sensitivities: c.calibrations.clone(),
                mics: c
                    .mics
                    .iter()
                    .map(|m| FileMic {
                        name: m.name.clone(),
                        curves: m
                            .curves
                            .iter()
                            .filter_map(|r| {
                                let id = MicCurveId {
                                    mic: m.name.clone(),
                                    label: r.label.clone(),
                                };
                                curves.get(&id).map(|p| FileCurve {
                                    reference: r.clone(),
                                    points: p
                                        .freqs()
                                        .iter()
                                        .zip(p.gains())
                                        .map(|(f, g)| [*f, *g])
                                        .collect(),
                                })
                            })
                            .collect(),
                    })
                    .collect(),
                inputs: c.inputs.clone(),
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
    pub(crate) fn curves(&self) -> Curves {
        self.curves.clone()
    }
}

fn parse(text: &[u8]) -> Result<(Contents, Curves), String> {
    let f: StoreFile = serde_json::from_slice(text).map_err(|e| e.to_string())?;
    if f.format != FORMAT || f.version != VERSION {
        return Err(format!(
            "format {:?} version {}, expected {FORMAT:?} version {VERSION}",
            f.format, f.version
        ));
    }
    let mut calibrations: Vec<CalEntry> = Vec::new();
    for e in f.sensitivities {
        if calibrations.iter().any(|x| x.key == e.key) {
            return Err(format!("duplicate calibration {:?}", e.key));
        }
        cal::check_mic_name(&e.key.mic)?;
        calibrations.push(e);
    }
    let mut mics: Vec<Mic> = Vec::new();
    let mut curves = HashMap::new();
    for m in f.mics {
        cal::check_mic_name(&m.name)?;
        if mics.iter().any(|x| x.name == m.name) {
            return Err(format!("mic {:?} listed twice", m.name));
        }
        if m.curves.is_empty() {
            return Err(format!("mic {:?} holds no curve", m.name));
        }
        let mut refs: Vec<MicCurveRef> = Vec::new();
        for c in m.curves {
            cal::check_label(&c.reference.label).map_err(|e| format!("mic {:?}: {e}", m.name))?;
            if refs.iter().any(|r| r.label == c.reference.label) {
                return Err(format!(
                    "mic {:?}: curve {:?} listed twice",
                    m.name, c.reference.label
                ));
            }
            let pts: Vec<(f64, f64)> = c.points.iter().map(|p| (p[0], p[1])).collect();
            let curve = MicCurve::from_points(&pts)
                .map_err(|err| format!("curve {:?} of {:?}: {err}", c.reference.label, m.name))?;
            curves.insert(
                MicCurveId {
                    mic: m.name.clone(),
                    label: c.reference.label.clone(),
                },
                Arc::new(curve),
            );
            refs.push(c.reference);
        }
        mics.push(Mic {
            name: m.name,
            curves: refs,
        });
    }
    mics.sort_by(|a, b| a.name.cmp(&b.name));
    let mut inputs = f.inputs;
    inputs.sort_by_key(|i| i.channel);
    if inputs.windows(2).any(|w| w[0].channel == w[1].channel) {
        return Err("duplicate input channel".into());
    }
    for i in &inputs {
        if let Some(m) = &i.mic {
            cal::check_mic_name(m)?;
        }
    }
    Ok((
        Contents {
            calibrations,
            mics,
            inputs,
        },
        curves,
    ))
}

/// The normalisation frequency of a sensitivity calibration in use: its calibrator
/// frequency (the tone was read uncorrected, so 0 dB there counts nothing twice).
pub(crate) fn f_norm(spl: Option<&CalEntry>) -> f64 {
    spl.map_or(DEFAULT_F_NORM, |e| e.spl.calibrator_freq.0)
}

/// What a job on `channel` of `device` uses (Q7 §3, `ac2_proto::cal::input_use`).
pub(crate) fn resolve(st: &State, store: &CalStore, device: &DeviceId, channel: u16) -> InputCal {
    let u: InputUse<'_> = cal::input_use(
        &st.calibrations,
        &st.mics,
        &st.inputs,
        Some(device),
        channel,
    );
    let spl = u.sensitivity.entry();
    let applied = match u.curve {
        cal::CurveUse::Applied { mic, curve } => {
            store.curve(mic, &curve.label).map(|p| (curve.clone(), p))
        }
        _ => None,
    };
    let f = f_norm(spl);
    InputCal {
        status: u.sensitivity.status(),
        sensitivity: spl.map(|e| e.spl.sensitivity.0),
        correction: applied.as_ref().map(|(_, p)| Arc::new(p.normalised(f))),
        spl_entry: spl.cloned(),
        curve: applied.map(|(r, _)| r),
    }
}

#[cfg(test)]
mod tests;
