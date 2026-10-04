//! UI preferences kept between runs (`ui.toml` in the ac2 config directory): the stimulus
//! outputs last used on each output device (decision K4), the session dialog's choices
//! per device — which inputs and outputs were in the session, their roles and the mic
//! names — the Leq view's layout, whether the panes show their key hints, how long the SPL
//! meter's number holds a reading, and the layout and window as last left: the focused
//! pane, maximised or full screen, what each pane shows, the window's size and position.
//!
//! ```toml
//! key_hints = false
//! spl_hold_ms = 250
//!
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
//!
//! [layout]
//! focus = "spl"
//! maximized = true
//! fullscreen = true
//! spl_view = "meter_leq"
//! ir_mode = "etc"
//! distortion_unit = "percent"
//!
//! [layout.measurements]
//! transfer = "Main L"
//! spl = "FOH SPL"
//!
//! [window]
//! width = 1600
//! height = 900
//! x = 80
//! y = 40
//! ```
//!
//! Channels are one-based in the file, as everywhere the operator reads or types them.
//! Measurements are remembered by name: ids do not outlive the daemon's state. The file is
//! UI state, not configuration: one that cannot be read is reported once and replaced by
//! the next save, and a write is atomic (`ac2_paths::write_private_atomic`).

use std::collections::BTreeMap;
use std::path::Path;

use ac2_scene::view::{DistortionUnit, IrMode, LeqLayout, LeqStyle, SplMode};
use serde::{Deserialize, Serialize};

use crate::state::PaneKind;

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

/// The layout as the operator left it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutPrefs {
    /// The focused pane.
    pub focus: PaneKind,
    /// Only the focused pane (W).
    pub maximized: bool,
    /// The window fills the screen (F11; with `maximized`, the full-screen pane).
    pub fullscreen: bool,
    /// What the SPL pane shows: the meter, the Leq windows or both.
    pub spl_view: SplMode,
    pub ir_mode: IrMode,
    pub distortion_unit: DistortionUnit,
    /// The measurement each pane shows, by name (transfer, spectrum, SPL).
    pub measurements: BTreeMap<PaneKind, String>,
}

impl Default for LayoutPrefs {
    fn default() -> Self {
        Self {
            focus: PaneKind::Transfer,
            maximized: false,
            fullscreen: false,
            spl_view: SplMode::MeterLeq,
            ir_mode: IrMode::Linear,
            distortion_unit: DistortionUnit::Db,
            measurements: BTreeMap::new(),
        }
    }
}

/// The window's size and position, logical points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowPrefs {
    pub width: u32,
    pub height: u32,
    /// Where its outer top-left was, when the system said (Wayland does not).
    pub pos: Option<(i32, i32)>,
}

impl WindowPrefs {
    /// Smallest window the app lays out.
    pub const MIN: (u32, u32) = (720, 480);
}

/// Bounds of the SPL meter's display hold, ms.
pub const SPL_HOLD_MS: std::ops::RangeInclusive<u32> = 100..=10_000;

/// What the UI remembers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiPrefs {
    /// Zero-based stimulus outputs per output device id.
    pub outputs: BTreeMap<String, Vec<u16>>,
    /// Session dialog choices per `backend/device id` ([`UiPrefs::device_key`]).
    pub sessions: BTreeMap<String, DeviceRoles>,
    /// How the SPL pane lays the Leq windows out.
    pub leq: LeqLayout,
    /// The focused pane's line of its most used keys (on until the operator turns it off).
    pub key_hints: bool,
    /// How long the SPL meter's number holds a reading; `None`: by its time weighting
    /// (`ac2_scene::spl::display_period_s`).
    pub spl_hold_ms: Option<u32>,
    /// The layout as last left.
    pub layout: LayoutPrefs,
    /// The window as last left (`None`: never saved).
    pub window: Option<WindowPrefs>,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            outputs: BTreeMap::new(),
            sessions: BTreeMap::new(),
            leq: LeqLayout::default(),
            key_hints: true,
            spl_hold_ms: None,
            layout: LayoutPrefs::default(),
            window: None,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    /// Written only when off (the default is on). First: plain values precede tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_hints: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spl_hold_ms: Option<u32>,
    /// One-based channels per device id.
    #[serde(default)]
    stimulus_outputs: BTreeMap<String, Vec<u32>>,
    /// Per `backend/device id`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    sessions: BTreeMap<String, RolesFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leq: Option<LeqFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    layout: Option<LayoutFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window: Option<WindowFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PaneFile {
    Transfer,
    Spectrum,
    Ir,
    Spl,
    Distortion,
}

impl PaneFile {
    fn of(p: PaneKind) -> Self {
        match p {
            PaneKind::Transfer => Self::Transfer,
            PaneKind::Spectrum => Self::Spectrum,
            PaneKind::Ir => Self::Ir,
            PaneKind::Spl => Self::Spl,
            PaneKind::Distortion => Self::Distortion,
        }
    }

    fn pane(self) -> PaneKind {
        match self {
            Self::Transfer => PaneKind::Transfer,
            Self::Spectrum => PaneKind::Spectrum,
            Self::Ir => PaneKind::Ir,
            Self::Spl => PaneKind::Spl,
            Self::Distortion => PaneKind::Distortion,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SplViewFile {
    Meter,
    Leq,
    MeterLeq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IrModeFile {
    Linear,
    Log,
    Etc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum UnitFile {
    Db,
    Percent,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeasurementsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transfer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spectrum: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spl: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutFile {
    focus: PaneFile,
    #[serde(default, skip_serializing_if = "is_false")]
    maximized: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    fullscreen: bool,
    #[serde(default = "spl_meter_leq")]
    spl_view: SplViewFile,
    #[serde(default = "ir_linear")]
    ir_mode: IrModeFile,
    #[serde(default = "unit_db")]
    distortion_unit: UnitFile,
    /// Last: a table.
    #[serde(default)]
    measurements: MeasurementsFile,
}

fn spl_meter_leq() -> SplViewFile {
    SplViewFile::MeterLeq
}

fn ir_linear() -> IrModeFile {
    IrModeFile::Linear
}

fn unit_db() -> UnitFile {
    UnitFile::Db
}

impl LayoutFile {
    fn parse(self) -> LayoutPrefs {
        let mut measurements = BTreeMap::new();
        for (p, name) in [
            (PaneKind::Transfer, self.measurements.transfer),
            (PaneKind::Spectrum, self.measurements.spectrum),
            (PaneKind::Spl, self.measurements.spl),
        ] {
            if let Some(n) = name {
                measurements.insert(p, n);
            }
        }
        LayoutPrefs {
            focus: self.focus.pane(),
            maximized: self.maximized,
            fullscreen: self.fullscreen,
            spl_view: match self.spl_view {
                SplViewFile::Meter => SplMode::Meter,
                SplViewFile::Leq => SplMode::Leq,
                SplViewFile::MeterLeq => SplMode::MeterLeq,
            },
            ir_mode: match self.ir_mode {
                IrModeFile::Linear => IrMode::Linear,
                IrModeFile::Log => IrMode::Log,
                IrModeFile::Etc => IrMode::Etc,
            },
            distortion_unit: match self.distortion_unit {
                UnitFile::Db => DistortionUnit::Db,
                UnitFile::Percent => DistortionUnit::Percent,
            },
            measurements,
        }
    }

    fn from_prefs(l: &LayoutPrefs) -> Self {
        let name = |p: PaneKind| l.measurements.get(&p).cloned();
        Self {
            focus: PaneFile::of(l.focus),
            maximized: l.maximized,
            fullscreen: l.fullscreen,
            spl_view: match l.spl_view {
                SplMode::Meter => SplViewFile::Meter,
                SplMode::Leq => SplViewFile::Leq,
                SplMode::MeterLeq => SplViewFile::MeterLeq,
            },
            ir_mode: match l.ir_mode {
                IrMode::Linear => IrModeFile::Linear,
                IrMode::Log => IrModeFile::Log,
                IrMode::Etc => IrModeFile::Etc,
            },
            distortion_unit: match l.distortion_unit {
                DistortionUnit::Db => UnitFile::Db,
                DistortionUnit::Percent => UnitFile::Percent,
            },
            measurements: MeasurementsFile {
                transfer: name(PaneKind::Transfer),
                spectrum: name(PaneKind::Spectrum),
                spl: name(PaneKind::Spl),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowFile {
    width: u32,
    height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    y: Option<i32>,
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
        if let Some(ms) = f.spl_hold_ms
            && !SPL_HOLD_MS.contains(&ms)
        {
            return Err(format!(
                "ui.toml: spl_hold_ms must be {} … {}",
                SPL_HOLD_MS.start(),
                SPL_HOLD_MS.end()
            ));
        }
        let window = match f.window {
            Some(w) if w.width >= WindowPrefs::MIN.0 && w.height >= WindowPrefs::MIN.1 => {
                Some(WindowPrefs {
                    width: w.width,
                    height: w.height,
                    pos: w.x.zip(w.y),
                })
            }
            Some(_) => {
                return Err(format!(
                    "ui.toml: the window must be at least {} × {}",
                    WindowPrefs::MIN.0,
                    WindowPrefs::MIN.1
                ));
            }
            None => None,
        };
        Ok(Self {
            outputs,
            sessions,
            leq,
            key_hints: f.key_hints.unwrap_or(true),
            spl_hold_ms: f.spl_hold_ms,
            layout: f.layout.map(LayoutFile::parse).unwrap_or_default(),
            window,
        })
    }

    /// The file text.
    pub fn to_toml(&self) -> String {
        let f = File {
            key_hints: (!self.key_hints).then_some(false),
            spl_hold_ms: self.spl_hold_ms,
            layout: (self.layout != LayoutPrefs::default())
                .then(|| LayoutFile::from_prefs(&self.layout)),
            window: self.window.map(|w| WindowFile {
                width: w.width,
                height: w.height,
                x: w.pos.map(|p| p.0),
                y: w.pos.map(|p| p.1),
            }),
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
    fn key_hints_round_trip() {
        let mut p = UiPrefs::default();
        assert!(p.key_hints);
        // On is the default and is not written.
        assert!(!p.to_toml().contains("key_hints"));
        p.key_hints = false;
        p.outputs.insert("fake:loop".into(), vec![0]);
        let text = p.to_toml();
        assert!(text.contains("key_hints = false"), "{text}");
        assert_eq!(UiPrefs::from_toml(&text), Ok(p));
        assert!(
            UiPrefs::from_toml("key_hints = true\n")
                .expect("parse")
                .key_hints
        );
        assert!(UiPrefs::from_toml("key_hints = \"no\"\n").is_err());
    }

    #[test]
    fn layout_window_and_hold_round_trip() {
        let mut p = UiPrefs::default();
        let text = p.to_toml();
        assert!(
            !text.contains("[layout]") && !text.contains("[window]"),
            "{text}"
        );
        p.spl_hold_ms = Some(250);
        p.layout = LayoutPrefs {
            focus: PaneKind::Spl,
            maximized: true,
            fullscreen: true,
            spl_view: SplMode::Leq,
            ir_mode: IrMode::Etc,
            distortion_unit: DistortionUnit::Percent,
            measurements: [
                (PaneKind::Transfer, "Main L".to_owned()),
                (PaneKind::Spl, "FOH SPL".to_owned()),
            ]
            .into(),
        };
        p.window = Some(WindowPrefs {
            width: 1600,
            height: 900,
            pos: Some((80, -20)),
        });
        let text = p.to_toml();
        for want in [
            "spl_hold_ms = 250",
            "[layout]",
            "focus = \"spl\"",
            "fullscreen = true",
            "spl_view = \"leq\"",
            "ir_mode = \"etc\"",
            "distortion_unit = \"percent\"",
            "[layout.measurements]",
            "spl = \"FOH SPL\"",
            "[window]",
            "y = -20",
        ] {
            assert!(text.contains(want), "{want} in {text}");
        }
        assert_eq!(UiPrefs::from_toml(&text), Ok(p.clone()));
        // No position (Wayland): size only.
        p.window = Some(WindowPrefs {
            width: 1000,
            height: 700,
            pos: None,
        });
        assert_eq!(UiPrefs::from_toml(&p.to_toml()), Ok(p));
        // A layout with only a focus takes the defaults for the rest.
        let q = UiPrefs::from_toml("[layout]\nfocus = \"ir\"\n").expect("parse");
        assert_eq!(
            q.layout,
            LayoutPrefs {
                focus: PaneKind::Ir,
                ..LayoutPrefs::default()
            }
        );
        for bad in [
            "spl_hold_ms = 5\n",
            "spl_hold_ms = 20000\n",
            "[layout]\nfocus = \"nowhere\"\n",
            "[layout]\nfocus = \"spl\"\nspl_view = \"bars\"\n",
            "[layout.measurements]\nir = \"x\"\n",
            "[window]\nwidth = 100\nheight = 100\n",
        ] {
            assert!(UiPrefs::from_toml(bad).is_err(), "{bad}");
        }
    }

    /// A new layout shows the meter with the Leq windows under it; a remembered choice
    /// stays, each one written as its name.
    #[test]
    fn spl_view_defaults_to_meter_and_leq_and_keeps_a_choice() {
        assert_eq!(LayoutPrefs::default().spl_view, SplMode::MeterLeq);
        let q = UiPrefs::from_toml("[layout]\nfocus = \"spl\"\n").expect("parse");
        assert_eq!(q.layout.spl_view, SplMode::MeterLeq);
        for (mode, name) in [
            (SplMode::Meter, "meter"),
            (SplMode::Leq, "leq"),
            (SplMode::MeterLeq, "meter_leq"),
        ] {
            let mut p = UiPrefs::default();
            p.layout.spl_view = mode;
            p.layout.focus = PaneKind::Spl;
            let text = p.to_toml();
            assert!(text.contains(&format!("spl_view = \"{name}\"")), "{text}");
            assert_eq!(
                UiPrefs::from_toml(&text).expect("parse").layout.spl_view,
                mode
            );
        }
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
