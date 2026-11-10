//! `ui.toml`'s per-rig input roles, pairs and remembered outputs.

use super::*;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RolesFile {
    #[serde(default)]
    inputs: Vec<u32>,
    #[serde(default)]
    outputs: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    spare_inputs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reference: Option<u32>,
    #[serde(default)]
    mics: Vec<u32>,
    #[serde(default)]
    stimulus: Vec<u32>,
    /// One-based channel (as text: TOML keys are strings) → mic name.
    #[serde(default)]
    mic_names: BTreeMap<String, String>,
    /// One-based channel (as text) → kind.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    kinds: BTreeMap<String, KindPref>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pairs: Vec<PairFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_device: Option<String>,
}

/// A left/right pair, one-based channels.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PairFile {
    left: u32,
    right: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

pub(super) fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// One-based → zero-based.
pub(super) fn zero(c: u32) -> Option<u16> {
    c.checked_sub(1).and_then(|c| u16::try_from(c).ok())
}

pub(super) fn zeros(v: &[u32]) -> Option<Vec<u16>> {
    v.iter().map(|c| zero(*c)).collect()
}

pub(super) fn ones(v: &[u16]) -> Vec<u32> {
    v.iter().map(|c| u32::from(*c) + 1).collect()
}

impl RolesFile {
    pub(super) fn parse(&self) -> Option<DeviceRoles> {
        let mut mic_names = BTreeMap::new();
        for (k, v) in &self.mic_names {
            mic_names.insert(zero(k.trim().parse().ok()?)?, v.clone());
        }
        let mut kinds = BTreeMap::new();
        for (k, v) in &self.kinds {
            kinds.insert(zero(k.trim().parse().ok()?)?, *v);
        }
        let mut pairs = Vec::new();
        for p in &self.pairs {
            pairs.push(ac2_proto::model::Pair {
                left: zero(p.left)?,
                right: zero(p.right)?,
                name: p.name.clone(),
            });
        }
        Some(DeviceRoles {
            inputs: zeros(&self.inputs)?,
            outputs: u16::try_from(self.outputs).ok()?,
            spare_inputs: u16::try_from(self.spare_inputs).ok()?,
            reference: match self.reference {
                Some(r) => Some(zero(r)?),
                None => None,
            },
            mics: zeros(&self.mics)?,
            stimulus: zeros(&self.stimulus)?,
            mic_names,
            kinds,
            pairs,
            output_device: self.output_device.clone(),
        })
    }

    pub(super) fn from_roles(r: &DeviceRoles) -> Self {
        Self {
            inputs: ones(&r.inputs),
            outputs: u32::from(r.outputs),
            spare_inputs: u32::from(r.spare_inputs),
            reference: r.reference.map(|c| u32::from(c) + 1),
            mics: ones(&r.mics),
            stimulus: ones(&r.stimulus),
            mic_names: r
                .mic_names
                .iter()
                .map(|(c, n)| ((u32::from(*c) + 1).to_string(), n.clone()))
                .collect(),
            kinds: r
                .kinds
                .iter()
                .map(|(c, k)| ((u32::from(*c) + 1).to_string(), *k))
                .collect(),
            pairs: r
                .pairs
                .iter()
                .map(|p| PairFile {
                    left: u32::from(p.left) + 1,
                    right: u32::from(p.right) + 1,
                    name: p.name.clone(),
                })
                .collect(),
            output_device: r.output_device.clone(),
        }
    }
}
