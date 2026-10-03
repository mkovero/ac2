//! UI preferences kept between runs (`ui.toml` in the ac2 config directory): the stimulus
//! outputs last used on each output device (decision K4), the session dialog's choices
//! per device — which inputs and outputs were in the session, their roles and the mic
//! names — and the Leq view's layout.
//!
//! ```toml
//! [stimulus_outputs]
//! "hw:UMC1820" = [1, 2]
//!
//! [sessions."cpal/hw:UMC1820"]
//! inputs = [1, 2, 3]
//! outputs = 2
//! reference = 1
//! mics = [2, 3]
//! stimulus = [1]
//! mic_names = { 2 = "M30 FOH", 3 = "ECM8000" }
//!
//! [leq]
//! style = "tiles"
//! history = true
//! ```
//!
//! Channels are one-based in the file, as everywhere the operator reads or types them. The
//! file is UI state, not configuration: one that cannot be read is reported once and
//! replaced by the next save, and a write is atomic (`ac2_paths::write_private_atomic`).

use std::collections::BTreeMap;
use std::path::Path;

use ac2_scene::view::{LeqLayout, LeqStyle};
use serde::{Deserialize, Serialize};

/// The session dialog's choices for one device (zero-based channels).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceRoles {
    /// Inputs in the session.
    pub inputs: Vec<u16>,
    /// Output channels of the session (a count: outputs `0 .. outputs`).
    pub outputs: u16,
    /// The loopback return.
    pub reference: Option<u16>,
    /// Measurement mics.
    pub mics: Vec<u16>,
    /// Stimulus outputs.
    pub stimulus: Vec<u16>,
    /// Mic name per input.
    pub mic_names: BTreeMap<u16, String>,
}

/// What the UI remembers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiPrefs {
    /// Zero-based stimulus outputs per output device id.
    pub outputs: BTreeMap<String, Vec<u16>>,
    /// Session dialog choices per `backend/device id` ([`UiPrefs::device_key`]).
    pub sessions: BTreeMap<String, DeviceRoles>,
    /// How the SPL pane lays the Leq windows out.
    pub leq: LeqLayout,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    /// One-based channels per device id.
    #[serde(default)]
    stimulus_outputs: BTreeMap<String, Vec<u32>>,
    /// Per `backend/device id`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    sessions: BTreeMap<String, RolesFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leq: Option<LeqFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StyleFile {
    Columns,
    Tiles,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeqFile {
    style: StyleFile,
    #[serde(default)]
    history: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RolesFile {
    #[serde(default)]
    inputs: Vec<u32>,
    #[serde(default)]
    outputs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reference: Option<u32>,
    #[serde(default)]
    mics: Vec<u32>,
    #[serde(default)]
    stimulus: Vec<u32>,
    /// One-based channel (as text: TOML keys are strings) → mic name.
    #[serde(default)]
    mic_names: BTreeMap<String, String>,
}

/// One-based → zero-based.
fn zero(c: u32) -> Option<u16> {
    c.checked_sub(1).and_then(|c| u16::try_from(c).ok())
}

fn zeros(v: &[u32]) -> Option<Vec<u16>> {
    v.iter().map(|c| zero(*c)).collect()
}

fn ones(v: &[u16]) -> Vec<u32> {
    v.iter().map(|c| u32::from(*c) + 1).collect()
}

impl RolesFile {
    fn parse(&self) -> Option<DeviceRoles> {
        let mut mic_names = BTreeMap::new();
        for (k, v) in &self.mic_names {
            mic_names.insert(zero(k.trim().parse().ok()?)?, v.clone());
        }
        Some(DeviceRoles {
            inputs: zeros(&self.inputs)?,
            outputs: u16::try_from(self.outputs).ok()?,
            reference: match self.reference {
                Some(r) => Some(zero(r)?),
                None => None,
            },
            mics: zeros(&self.mics)?,
            stimulus: zeros(&self.stimulus)?,
            mic_names,
        })
    }

    fn from_roles(r: &DeviceRoles) -> Self {
        Self {
            inputs: ones(&r.inputs),
            outputs: u32::from(r.outputs),
            reference: r.reference.map(|c| u32::from(c) + 1),
            mics: ones(&r.mics),
            stimulus: ones(&r.stimulus),
            mic_names: r
                .mic_names
                .iter()
                .map(|(c, n)| ((u32::from(*c) + 1).to_string(), n.clone()))
                .collect(),
        }
    }
}

impl UiPrefs {
    /// The outputs remembered for `device`.
    pub fn outputs_for(&self, device: &str) -> Option<&[u16]> {
        self.outputs.get(device).map(Vec::as_slice)
    }

    /// Key of a device in [`UiPrefs::sessions`]: `fake/fake:loop`, `cpal/hw:UMC1820`.
    pub fn device_key(backend: ac2_proto::model::BackendKind, device: &str) -> String {
        use ac2_proto::model::BackendKind as B;
        let b = match backend {
            B::Jack => "jack",
            B::Cpal => "cpal",
            B::Fake => "fake",
        };
        format!("{b}/{device}")
    }

    /// Parses `ui.toml`.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let f: File = toml::from_str(text).map_err(|e| format!("ui.toml: {e}"))?;
        let mut outputs = BTreeMap::new();
        for (device, chans) in f.stimulus_outputs {
            match zeros(&chans) {
                Some(z) if !z.is_empty() => {
                    outputs.insert(device, z);
                }
                _ => {
                    return Err(format!(
                        "ui.toml: stimulus outputs of {device:?} must be channels from 1"
                    ));
                }
            }
        }
        let mut sessions = BTreeMap::new();
        for (device, r) in f.sessions {
            let roles = r.parse().ok_or_else(|| {
                format!("ui.toml: session choices of {device:?} must be channels from 1")
            })?;
            sessions.insert(device, roles);
        }
        let leq = f.leq.map_or_else(LeqLayout::default, |l| LeqLayout {
            style: match l.style {
                StyleFile::Columns => LeqStyle::Columns,
                StyleFile::Tiles => LeqStyle::Tiles,
            },
            history: l.history,
        });
        Ok(Self {
            outputs,
            sessions,
            leq,
        })
    }

    /// The file text.
    pub fn to_toml(&self) -> String {
        let f = File {
            stimulus_outputs: self
                .outputs
                .iter()
                .map(|(d, o)| (d.clone(), ones(o)))
                .collect(),
            sessions: self
                .sessions
                .iter()
                .map(|(d, r)| (d.clone(), RolesFile::from_roles(r)))
                .collect(),
            leq: (self.leq != LeqLayout::default()).then_some(LeqFile {
                style: match self.leq.style {
                    LeqStyle::Columns => StyleFile::Columns,
                    LeqStyle::Tiles => StyleFile::Tiles,
                },
                history: self.leq.history,
            }),
        };
        toml::to_string(&f).unwrap_or_default()
    }

    /// Reads `path`. A missing file is the defaults; an unreadable one is the defaults plus
    /// the reason, shown once (the next save replaces it).
    pub fn load(path: Option<&Path>) -> (Self, Option<String>) {
        let Some(path) = path else {
            return (Self::default(), None);
        };
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::from_toml(&text) {
                Ok(p) => (p, None),
                Err(e) => (
                    Self::default(),
                    Some(format!("{e} ({}); using defaults", path.display())),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self::default(),
                Some(format!("cannot read {}: {e}", path.display())),
            ),
        }
    }

    /// Writes `path` atomically.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        ac2_paths::write_private_atomic(path, self.to_toml().as_bytes())
            .map_err(|e| format!("cannot save {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_is_one_based_in_the_file() {
        let mut p = UiPrefs::default();
        p.outputs.insert("hw:UMC1820".into(), vec![0, 1]);
        p.outputs.insert("fake:loop".into(), vec![3]);
        p.sessions.insert(
            "cpal/hw:UMC1820".into(),
            DeviceRoles {
                inputs: vec![0, 1, 2],
                outputs: 2,
                reference: Some(0),
                mics: vec![1, 2],
                stimulus: vec![0],
                mic_names: [(1, "M30 FOH".to_owned()), (2, "ECM8000".to_owned())].into(),
            },
        );
        let text = p.to_toml();
        assert!(text.contains("\"hw:UMC1820\" = [1, 2]"), "{text}");
        assert!(text.contains("reference = 1"), "{text}");
        assert!(text.contains("mics = [2, 3]"), "{text}");
        assert!(text.contains("2 = \"M30 FOH\""), "{text}");
        assert_eq!(UiPrefs::from_toml(&text), Ok(p));
        assert_eq!(UiPrefs::from_toml(""), Ok(UiPrefs::default()));
    }

    #[test]
    fn leq_layout_round_trips() {
        let mut p = UiPrefs::default();
        assert_eq!(
            p.leq,
            LeqLayout {
                style: LeqStyle::Columns,
                history: false
            }
        );
        // The default is not written.
        assert!(!p.to_toml().contains("[leq]"));
        p.leq = LeqLayout {
            style: LeqStyle::Tiles,
            history: true,
        };
        let text = p.to_toml();
        assert!(text.contains("[leq]"), "{text}");
        assert!(text.contains("style = \"tiles\""), "{text}");
        assert!(text.contains("history = true"), "{text}");
        assert_eq!(UiPrefs::from_toml(&text), Ok(p));
        let q = UiPrefs::from_toml("[leq]\nstyle = \"columns\"\nhistory = true\n").expect("parse");
        assert_eq!(q.leq.style, LeqStyle::Columns);
        assert!(q.leq.history);
        assert!(UiPrefs::from_toml("[leq]\nstyle = \"bars\"\n").is_err());
    }

    #[test]
    fn bad_files_are_reported() {
        assert!(UiPrefs::from_toml("[stimulus_outputs]\nx = [0]\n").is_err());
        assert!(UiPrefs::from_toml("[stimulus_outputs]\nx = []\n").is_err());
        assert!(UiPrefs::from_toml("colour = 1\n").is_err());
        assert!(UiPrefs::from_toml("[sessions.\"fake/x\"]\nmics = [0]\n").is_err());
        assert!(UiPrefs::from_toml("[sessions.\"fake/x\"]\nmic_names = { a = \"M\" }\n").is_err());
        assert!(UiPrefs::from_toml("[sessions.\"fake/x\"]\ncolour = 1\n").is_err());
    }

    #[test]
    fn load_and_atomic_save() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ac2").join("ui.toml");
        assert_eq!(UiPrefs::load(Some(&path)), (UiPrefs::default(), None));
        let mut p = UiPrefs::default();
        p.outputs.insert("fake:loop".into(), vec![1]);
        p.save(&path).expect("save");
        assert_eq!(UiPrefs::load(Some(&path)), (p.clone(), None));
        // Only the file itself is left in the directory.
        let names: Vec<_> = std::fs::read_dir(path.parent().expect("dir"))
            .expect("list")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        std::fs::write(&path, "garbage =").expect("write");
        let (q, err) = UiPrefs::load(Some(&path));
        assert_eq!(q, UiPrefs::default());
        assert!(err.is_some_and(|e| e.contains("ui.toml")));
    }
}
