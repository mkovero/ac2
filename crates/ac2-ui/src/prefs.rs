//! UI preferences kept between runs (`ui.toml` in the ac2 config directory): the stimulus
//! outputs last used on each output device (decision K4).
//!
//! ```toml
//! [stimulus_outputs]
//! "hw:UMC1820" = [1, 2]
//! ```
//!
//! Channels are one-based in the file, as everywhere the operator reads or types them. The
//! file is UI state, not configuration: one that cannot be read is reported once and
//! replaced by the next save, and a write is atomic (`ac2_paths::write_private_atomic`).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// What the UI remembers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiPrefs {
    /// Zero-based stimulus outputs per output device id.
    pub outputs: BTreeMap<String, Vec<u16>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    /// One-based channels per device id.
    #[serde(default)]
    stimulus_outputs: BTreeMap<String, Vec<u32>>,
}

impl UiPrefs {
    /// The outputs remembered for `device`.
    pub fn outputs_for(&self, device: &str) -> Option<&[u16]> {
        self.outputs.get(device).map(Vec::as_slice)
    }

    /// Parses `ui.toml`.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let f: File = toml::from_str(text).map_err(|e| format!("ui.toml: {e}"))?;
        let mut outputs = BTreeMap::new();
        for (device, chans) in f.stimulus_outputs {
            let zero: Option<Vec<u16>> = chans
                .iter()
                .map(|c| c.checked_sub(1).and_then(|c| u16::try_from(c).ok()))
                .collect();
            match zero {
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
        Ok(Self { outputs })
    }

    /// The file text.
    pub fn to_toml(&self) -> String {
        let f = File {
            stimulus_outputs: self
                .outputs
                .iter()
                .map(|(d, o)| (d.clone(), o.iter().map(|c| u32::from(*c) + 1).collect()))
                .collect(),
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
        let text = p.to_toml();
        assert!(text.contains("\"hw:UMC1820\" = [1, 2]"), "{text}");
        assert_eq!(UiPrefs::from_toml(&text), Ok(p));
        assert_eq!(UiPrefs::from_toml(""), Ok(UiPrefs::default()));
    }

    #[test]
    fn bad_files_are_reported() {
        assert!(UiPrefs::from_toml("[stimulus_outputs]\nx = [0]\n").is_err());
        assert!(UiPrefs::from_toml("[stimulus_outputs]\nx = []\n").is_err());
        assert!(UiPrefs::from_toml("colour = 1\n").is_err());
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
