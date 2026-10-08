//! UI preferences kept between runs (`ui.toml` in the ac2 config directory): the stimulus
//! outputs last used on each output device (decision K4), the session dialog's choices
//! per device — which inputs and outputs were in the session, their roles and the mic
//! names — the Leq view's layout, whether the panes show their key hints, how long the SPL
//! meter's number holds a reading, the theme, the record toggle's time limit, the
//! spectrograph's history span, the layout and window as last left: the focused
//! pane, maximised or full screen, what each pane shows, the window's size and position,
//! and each pane's level axis range (a fit made for one show is a fair start for the next).
//!
//! ```toml
//! key_hints = false
//! spl_hold_ms = 250
//! theme = "light"
//! record_limit_min = 90
//! spectrograph_span_s = 30
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
//! spectrum_view = "spectrograph"
//! sweep_view = "room"
//! ir_mode = "etc"
//! distortion_unit = "percent"
//! hidden = ["TF 2"]
//!
//! [layout.measurements]
//! transfer = "Main L"
//! spl = "FOH SPL"
//!
//! [levels]
//! transfer = [-24.0, 12.0]
//! spectrum_dbfs = [-140.0, -40.0]
//! spectrum_spl = [20.0, 120.0]
//! distortion = [-100.0, 0.0]
//! ir = [-80.0, 3.0]
//! sweep_ir = [-90.0, 0.0]
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

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ac2_scene::axis::Range;
use ac2_scene::view::{
    DistortionUnit, IrMode, LeqLayout, LeqStyle, SpectrumMode, SplMode, SweepMode, ViewState, level,
};
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
    /// The window fills the screen (F or F11; with `maximized`, the full-screen pane).
    pub fullscreen: bool,
    /// What the SPL pane shows: the meter, the Leq windows or both.
    pub spl_view: SplMode,
    /// What the spectrum pane shows: the spectrum, the spectrograph or both.
    pub spectrum_view: SpectrumMode,
    /// What the sweep pane shows: the response and distortion, the IR or the room table.
    pub sweep_view: SweepMode,
    pub ir_mode: IrMode,
    pub distortion_unit: DistortionUnit,
    /// The measurement each pane shows, by name (transfer, spectrum, SPL).
    pub measurements: BTreeMap<PaneKind, String>,
    /// Measurements whose live curves are hidden (A), by name.
    pub hidden: BTreeSet<String>,
}

impl Default for LayoutPrefs {
    fn default() -> Self {
        Self {
            focus: PaneKind::Transfer,
            maximized: false,
            fullscreen: false,
            spl_view: SplMode::MeterLeq,
            spectrum_view: SpectrumMode::Spectrum,
            sweep_view: SweepMode::Response,
            ir_mode: IrMode::Linear,
            distortion_unit: DistortionUnit::Db,
            measurements: BTreeMap::new(),
            hidden: BTreeSet::new(),
        }
    }
}

/// The level axis range of each pane, dB (the spectrum pane keeps one per scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelPrefs {
    pub transfer: Range,
    pub spectrum_dbfs: Range,
    pub spectrum_spl: Range,
    pub distortion: Range,
    /// The IR pane's log / ETC axis, dB re peak.
    pub ir: Range,
    /// The sweep pane's IR view's log / ETC axis, dB re peak.
    pub sweep_ir: Range,
}

impl LevelPrefs {
    /// The ranges `view` shows.
    pub fn of(view: &ViewState) -> Self {
        Self {
            transfer: view.tf.magnitude_db,
            spectrum_dbfs: view.spectrum.level,
            spectrum_spl: view.spectrum.level_spl,
            distortion: view.distortion.range_db,
            ir: view.ir.axes.level_db,
            sweep_ir: view.distortion.ir.level_db,
        }
    }

    /// Puts the ranges on `view`.
    pub fn apply(&self, view: &mut ViewState) {
        view.tf.magnitude_db = self.transfer;
        view.spectrum.level = self.spectrum_dbfs;
        view.spectrum.level_spl = self.spectrum_spl;
        view.distortion.range_db = self.distortion;
        view.ir.axes.level_db = self.ir;
        view.distortion.ir.level_db = self.sweep_ir;
    }
}

impl Default for LevelPrefs {
    fn default() -> Self {
        Self::of(&ViewState::default())
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
#[derive(Clone, Debug, PartialEq)]
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
    /// The level axes as last left.
    pub levels: LevelPrefs,
    /// The window as last left (`None`: never saved).
    pub window: Option<WindowPrefs>,
    /// The theme last chosen (`None`: never chosen; `ac2-ui --theme` overrides it for a run).
    pub theme: Option<ac2_scene::theme::ThemeName>,
    /// How long the record toggle records before it stops by itself, minutes (`None`: an
    /// hour, [`crate::state::RECORD_MAX_S`]).
    pub record_limit_min: Option<u32>,
    /// The spectrograph's history span, s (one of
    /// [`ac2_scene::view::SPECTROGRAPH_SPANS_S`]; `None`: its default).
    pub spectrograph_span_s: Option<u32>,
}

/// Bounds of the record toggle's limit, minutes: a whole day of every input of a large
/// interface would fill a disk unattended.
pub const RECORD_LIMIT_MIN: std::ops::RangeInclusive<u32> = 1..=480;

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            outputs: BTreeMap::new(),
            sessions: BTreeMap::new(),
            leq: LeqLayout::default(),
            key_hints: true,
            spl_hold_ms: None,
            layout: LayoutPrefs::default(),
            levels: LevelPrefs::default(),
            window: None,
            theme: None,
            record_limit_min: None,
            spectrograph_span_s: None,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme: Option<ThemeFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    record_limit_min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spectrograph_span_s: Option<u32>,
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
    levels: Option<LevelsFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window: Option<WindowFile>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ThemeFile {
    Dark,
    Light,
    HighContrast,
}

impl ThemeFile {
    fn name(self) -> ac2_scene::theme::ThemeName {
        use ac2_scene::theme::ThemeName as T;
        match self {
            Self::Dark => T::Dark,
            Self::Light => T::Light,
            Self::HighContrast => T::HighContrast,
        }
    }

    fn of(t: ac2_scene::theme::ThemeName) -> Self {
        use ac2_scene::theme::ThemeName as T;
        match t {
            T::Dark => Self::Dark,
            T::Light => Self::Light,
            T::HighContrast => Self::HighContrast,
        }
    }
}

/// `[low, high]` dB per axis; one left out is the default.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LevelsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transfer: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spectrum_dbfs: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spectrum_spl: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    distortion: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ir: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sweep_ir: Option<[f64; 2]>,
}

impl LevelsFile {
    fn parse(&self) -> Result<LevelPrefs, String> {
        let d = LevelPrefs::default();
        let range = |name: &str, v: Option<[f64; 2]>, default: Range| match v {
            None => Ok(default),
            Some([lo, hi]) if lo.is_finite() && hi.is_finite() && lo < hi => {
                Ok(level::clamp(Range::new(lo, hi)))
            }
            Some(_) => Err(format!(
                "ui.toml: levels.{name} must be [low, high] in dB, low below high"
            )),
        };
        Ok(LevelPrefs {
            transfer: range("transfer", self.transfer, d.transfer)?,
            spectrum_dbfs: range("spectrum_dbfs", self.spectrum_dbfs, d.spectrum_dbfs)?,
            spectrum_spl: range("spectrum_spl", self.spectrum_spl, d.spectrum_spl)?,
            distortion: range("distortion", self.distortion, d.distortion)?,
            ir: range("ir", self.ir, d.ir)?,
            sweep_ir: range("sweep_ir", self.sweep_ir, d.sweep_ir)?,
        })
    }

    /// Only the ranges moved off their defaults.
    fn from_prefs(l: &LevelPrefs) -> Option<Self> {
        let d = LevelPrefs::default();
        let v = |r: Range, default: Range| (r != default).then_some([r.lo, r.hi]);
        let f = Self {
            transfer: v(l.transfer, d.transfer),
            spectrum_dbfs: v(l.spectrum_dbfs, d.spectrum_dbfs),
            spectrum_spl: v(l.spectrum_spl, d.spectrum_spl),
            distortion: v(l.distortion, d.distortion),
            ir: v(l.ir, d.ir),
            sweep_ir: v(l.sweep_ir, d.sweep_ir),
        };
        (*l != d).then_some(f)
    }
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
    Bands,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SpectrumViewFile {
    #[default]
    Spectrum,
    SpectrumSpectrograph,
    Spectrograph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SweepViewFile {
    #[default]
    Response,
    Ir,
    Room,
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
    #[serde(default)]
    spectrum_view: SpectrumViewFile,
    #[serde(default)]
    sweep_view: SweepViewFile,
    #[serde(default = "ir_linear")]
    ir_mode: IrModeFile,
    #[serde(default = "unit_db")]
    distortion_unit: UnitFile,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hidden: Vec<String>,
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
                SplViewFile::Bands => SplMode::Bands,
            },
            spectrum_view: match self.spectrum_view {
                SpectrumViewFile::Spectrum => SpectrumMode::Spectrum,
                SpectrumViewFile::SpectrumSpectrograph => SpectrumMode::Split,
                SpectrumViewFile::Spectrograph => SpectrumMode::Spectrograph,
            },
            sweep_view: match self.sweep_view {
                SweepViewFile::Response => SweepMode::Response,
                SweepViewFile::Ir => SweepMode::Ir,
                SweepViewFile::Room => SweepMode::Room,
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
            hidden: self.hidden.into_iter().collect(),
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
                SplMode::Bands => SplViewFile::Bands,
            },
            spectrum_view: match l.spectrum_view {
                SpectrumMode::Spectrum => SpectrumViewFile::Spectrum,
                SpectrumMode::Split => SpectrumViewFile::SpectrumSpectrograph,
                SpectrumMode::Spectrograph => SpectrumViewFile::Spectrograph,
            },
            sweep_view: match l.sweep_view {
                SweepMode::Response => SweepViewFile::Response,
                SweepMode::Ir => SweepViewFile::Ir,
                SweepMode::Room => SweepViewFile::Room,
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
            hidden: l.hidden.iter().cloned().collect(),
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
            B::Replay => "replay",
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
        if let Some(m) = f.record_limit_min
            && !RECORD_LIMIT_MIN.contains(&m)
        {
            return Err(format!(
                "ui.toml: record_limit_min must be {} … {}",
                RECORD_LIMIT_MIN.start(),
                RECORD_LIMIT_MIN.end()
            ));
        }
        if let Some(s) = f.spectrograph_span_s
            && !ac2_scene::view::SPECTROGRAPH_SPANS_S.contains(&s)
        {
            return Err(format!(
                "ui.toml: spectrograph_span_s must be one of {:?}",
                ac2_scene::view::SPECTROGRAPH_SPANS_S
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
            levels: f
                .levels
                .as_ref()
                .map(LevelsFile::parse)
                .transpose()?
                .unwrap_or_default(),
            window,
            theme: f.theme.map(ThemeFile::name),
            record_limit_min: f.record_limit_min,
            spectrograph_span_s: f.spectrograph_span_s,
        })
    }

    /// The file text.
    pub fn to_toml(&self) -> String {
        let f = File {
            key_hints: (!self.key_hints).then_some(false),
            spl_hold_ms: self.spl_hold_ms,
            theme: self.theme.map(ThemeFile::of),
            record_limit_min: self.record_limit_min,
            spectrograph_span_s: self.spectrograph_span_s,
            layout: (self.layout != LayoutPrefs::default())
                .then(|| LayoutFile::from_prefs(&self.layout)),
            levels: LevelsFile::from_prefs(&self.levels),
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
            spectrum_view: SpectrumMode::Spectrograph,
            sweep_view: SweepMode::Room,
            ir_mode: IrMode::Etc,
            distortion_unit: DistortionUnit::Percent,
            measurements: [
                (PaneKind::Transfer, "Main L".to_owned()),
                (PaneKind::Spl, "FOH SPL".to_owned()),
            ]
            .into(),
            hidden: ["TF 2".to_owned()].into(),
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
            "spectrum_view = \"spectrograph\"",
            "sweep_view = \"room\"",
            "ir_mode = \"etc\"",
            "distortion_unit = \"percent\"",
            "hidden = [\"TF 2\"]",
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
            (SplMode::Bands, "bands"),
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

    /// The spectrum pane starts on the spectrum; a remembered view stays, by name.
    #[test]
    fn spectrum_view_defaults_to_the_spectrum_and_keeps_a_choice() {
        assert_eq!(LayoutPrefs::default().spectrum_view, SpectrumMode::Spectrum);
        for (mode, name) in [
            (SpectrumMode::Spectrum, "spectrum"),
            (SpectrumMode::Split, "spectrum_spectrograph"),
            (SpectrumMode::Spectrograph, "spectrograph"),
        ] {
            let mut p = UiPrefs::default();
            p.layout.spectrum_view = mode;
            p.layout.focus = PaneKind::Spectrum;
            let text = p.to_toml();
            assert!(
                text.contains(&format!("spectrum_view = \"{name}\"")),
                "{text}"
            );
            assert_eq!(
                UiPrefs::from_toml(&text)
                    .expect("parse")
                    .layout
                    .spectrum_view,
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
        assert!(UiPrefs::from_toml("[levels]\ntransfer = [10.0, -10.0]\n").is_err());
        assert!(UiPrefs::from_toml("[levels]\ntransfer = [-10.0]\n").is_err());
        assert!(UiPrefs::from_toml("[levels]\nphase = [-10.0, 10.0]\n").is_err());
    }

    /// Level axes moved off their defaults are written, the rest left out; an
    /// out-of-limits range is brought within the axis limits on reading.
    #[test]
    fn level_ranges_round_trip() {
        let d = UiPrefs::default();
        assert!(!d.to_toml().contains("levels"));
        let mut p = UiPrefs::default();
        p.levels.spectrum_dbfs = Range::new(-140.0, -40.0);
        p.levels.transfer = Range::new(-24.5, 12.0);
        p.levels.sweep_ir = Range::new(-90.0, 0.0);
        let text = p.to_toml();
        assert!(text.contains("sweep_ir = [-90.0, 0.0]"), "{text}");
        assert!(!text.contains("\nir ="), "{text}");
        assert!(text.contains("[levels]"), "{text}");
        assert!(text.contains("spectrum_dbfs = [-140.0, -40.0]"), "{text}");
        assert!(!text.contains("spectrum_spl"), "{text}");
        assert!(!text.contains("distortion ="), "{text}");
        assert_eq!(UiPrefs::from_toml(&text), Ok(p));
        let far = UiPrefs::from_toml("[levels]\ndistortion = [-900.0, -800.0]\n").expect("parse");
        assert_eq!(far.levels.distortion, Range::new(-300.0, -200.0));
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
