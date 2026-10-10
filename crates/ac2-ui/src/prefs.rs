//! UI preferences kept between runs (`ui.toml` in the ac2 config directory): the stimulus
//! outputs last used on each output device (decision K4), the session dialog's choices
//! per device — which inputs and outputs were in the session, their roles and the mic
//! names — the Leq view's layout, whether the panes show their key hints, how long the SPL
//! meter's number holds a reading, the theme, the record toggle's time limit, the
//! spectrograph's history span, the layout and window as last left: the pane tree, the
//! focused pane, maximised or full screen, what each pane shows (its kind, measurement,
//! modes and how much of its plot: grid, labels, cursor), the window's size and position,
//! and each pane's level axis range (a fit made for one show is a fair start for the next),
//! and where the transfer pane's legend sat and how large it may be.
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
//! focus = 2
//! maximized = true
//! fullscreen = true
//! distortion_unit = "percent"
//! hidden = ["TF 2"]
//! compared = ["Main R"]
//!
//! [layout.tree]
//! split = "row"
//! ratio = 0.5
//! a = { pane = 1 }
//! b = { pane = 2 }
//!
//! [[layout.panes]]
//! id = 1
//! kind = "transfer"
//! measurement = "Main L"
//!
//! [[layout.panes]]
//! id = 2
//! kind = "spl"
//! measurement = "FOH SPL"
//! spl_view = "meter_leq"
//! chrome = "no_grid"
//!
//! [levels]
//! transfer = [-24.0, 12.0]
//! spectrum_dbfs = [-140.0, -40.0]
//! spectrum_spl = [20.0, 120.0]
//! distortion = [-100.0, 0.0]
//! ir = [-80.0, 3.0]
//! sweep_ir = [-90.0, 0.0]
//!
//! [legend.transfer]
//! x = 1.0
//! y = 0.0
//! max_height = 0.3
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
use ac2_scene::legend::LegendView;
use ac2_scene::view::{
    DistortionUnit, IrMode, LeqLayout, LeqStyle, PlotChrome, SpectrumMode, SplMode, SweepMode,
    ViewState, level,
};
use serde::{Deserialize, Serialize};

use crate::state::{Axis, PaneId, PaneKind, PaneModes, PaneNode, TransferView};

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
    /// The device the session plays on when it is not the input device (its id); `None`:
    /// the input device's own outputs, or the system default where it has none.
    pub output_device: Option<String>,
}

/// The layout as the operator left it.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutPrefs {
    /// The pane tree and what each pane shows; `None`: none remembered, the app starts with
    /// one pane.
    pub panes: Option<PanesPrefs>,
    /// Only the focused pane (W).
    pub maximized: bool,
    /// The window fills the screen (F or F11; with `maximized`, the full-screen pane).
    pub fullscreen: bool,
    pub distortion_unit: DistortionUnit,
    /// Measurements whose live curves are hidden (A), by name.
    pub hidden: BTreeSet<String>,
    /// Measurements whose live curves every transfer pane draws besides its own group
    /// (compare, C), by name.
    pub compared: BTreeSet<String>,
}

impl Default for LayoutPrefs {
    fn default() -> Self {
        Self {
            panes: None,
            maximized: false,
            fullscreen: false,
            distortion_unit: DistortionUnit::Db,
            hidden: BTreeSet::new(),
            compared: BTreeSet::new(),
        }
    }
}

/// The pane tree as left.
#[derive(Clone, Debug, PartialEq)]
pub struct PanesPrefs {
    pub root: PaneNode,
    pub focus: PaneId,
    /// One per leaf.
    pub views: Vec<PanePrefs>,
}

/// What one pane showed.
#[derive(Clone, Debug, PartialEq)]
pub struct PanePrefs {
    pub id: PaneId,
    pub kind: PaneKind,
    /// The measurement it showed, by name.
    pub measurement: Option<String>,
    pub modes: PaneModes,
}

/// The level axis range of each pane, dB (the spectrum pane keeps one per scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelPrefs {
    pub transfer: Range,
    pub spectrum_dbfs: Range,
    pub spectrum_spl: Range,
    pub distortion: Range,
    /// The transfer pane's IR view's log / ETC axis, dB re peak.
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
    /// Warnings and Leq limit alarms pop up in the corner (on until the operator turns it
    /// off); off, they go only to the notification log.
    pub warning_toasts: bool,
    /// The transfer and sweep panes shade the ranges a curve's analysis does not resolve
    /// at its grid (on until the operator turns it off).
    pub resolution_marker: bool,
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
    /// The transfer pane's legend as last left.
    pub legend: LegendPrefs,
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
            warning_toasts: true,
            resolution_marker: true,
            spl_hold_ms: None,
            layout: LayoutPrefs::default(),
            levels: LevelPrefs::default(),
            window: None,
            theme: None,
            record_limit_min: None,
            spectrograph_span_s: None,
            legend: LegendPrefs::default(),
        }
    }
}

/// A legend as last left: hidden or not, where it sat, how large it may be (the scroll and
/// the pointer are not kept).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegendPrefs {
    pub hidden: bool,
    pub x: f32,
    pub y: f32,
    pub max_width: f32,
    pub max_height: f32,
}

impl LegendPrefs {
    pub fn of(v: &LegendView) -> Self {
        Self {
            hidden: v.hidden,
            x: v.x,
            y: v.y,
            max_width: v.max_width,
            max_height: v.max_height,
        }
    }

    /// Puts the choices on `v`.
    pub fn apply(&self, v: &mut LegendView) {
        v.hidden = self.hidden;
        v.move_to(self.x, self.y);
        v.resize(self.max_width, self.max_height);
    }
}

impl Default for LegendPrefs {
    fn default() -> Self {
        Self::of(&LegendView::default())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    /// Written only when off (the default is on). First: plain values precede tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_hints: Option<bool>,
    /// Written only when off (the default is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    warning_toasts: Option<bool>,
    /// Written only when off (the default is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolution_marker: Option<bool>,
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
    /// A `[layout]` that does not parse as a pane tree is dropped (with a warning in the
    /// log): the panes are a convenience, so the app starts with one pane instead of
    /// refusing the hold times, sessions and devices the rest of the file keeps.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_layout"
    )]
    layout: Option<LayoutFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    levels: Option<LevelsFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legend: Option<LegendsFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window: Option<WindowFile>,
    /// Top-level keys this version does not know: logged and left out of the next save, so
    /// one stale or mistyped setting costs only itself, never the rest of the file.
    #[serde(flatten, skip_serializing)]
    unknown: BTreeMap<String, toml::Value>,
}

/// Legends per pane; only the transfer pane's moves.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegendsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transfer: Option<LegendFile>,
}

/// A legend's choices; one left out is the default.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegendFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_height: Option<f64>,
}

impl LegendFile {
    fn parse(&self) -> Result<LegendPrefs, String> {
        let d = LegendPrefs::default();
        let limits = ac2_scene::legend::SIZE_LIMITS;
        let (lo, hi) = (f64::from(*limits.start()), f64::from(*limits.end()));
        let num = |name: &str, v: Option<f64>, default: f32, lo: f64, hi: f64| match v {
            None => Ok(default),
            Some(v) if (lo..=hi).contains(&v) => Ok(v as f32),
            Some(_) => Err(format!(
                "ui.toml: legend.transfer.{name} must be {lo} … {hi}"
            )),
        };
        Ok(LegendPrefs {
            hidden: self.hidden.unwrap_or(d.hidden),
            x: num("x", self.x, d.x, 0.0, 1.0)?,
            y: num("y", self.y, d.y, 0.0, 1.0)?,
            max_width: num("max_width", self.max_width, d.max_width, lo, hi)?,
            max_height: num("max_height", self.max_height, d.max_height, lo, hi)?,
        })
    }

    /// Only the choices moved off their defaults, to three decimals (a drag's fraction
    /// has no meaningful digits beyond a pixel).
    fn from_prefs(l: &LegendPrefs) -> Option<LegendsFile> {
        let d = LegendPrefs::default();
        let v =
            |x: f32, default: f32| (x != default).then(|| (f64::from(x) * 1000.0).round() / 1000.0);
        let f = Self {
            hidden: l.hidden.then_some(true),
            x: v(l.x, d.x),
            y: v(l.y, d.y),
            max_width: v(l.max_width, d.max_width),
            max_height: v(l.max_height, d.max_height),
        };
        (*l != d).then_some(LegendsFile { transfer: Some(f) })
    }
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
    Spl,
    Distortion,
}

impl PaneFile {
    fn of(p: PaneKind) -> Self {
        match p {
            PaneKind::Transfer => Self::Transfer,
            PaneKind::Spectrum => Self::Spectrum,
            PaneKind::Spl => Self::Spl,
            PaneKind::Distortion => Self::Distortion,
        }
    }

    fn pane(self) -> PaneKind {
        match self {
            Self::Transfer => PaneKind::Transfer,
            Self::Spectrum => PaneKind::Spectrum,
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
enum TransferViewFile {
    #[default]
    Response,
    Phase,
    Coherence,
    Ir,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ChromeFile {
    #[default]
    Full,
    NoGrid,
    TracesOnly,
}

impl ChromeFile {
    fn of(c: PlotChrome) -> Self {
        match c {
            PlotChrome::Full => Self::Full,
            PlotChrome::NoGrid => Self::NoGrid,
            PlotChrome::Bare => Self::TracesOnly,
        }
    }

    fn chrome(self) -> PlotChrome {
        match self {
            Self::Full => PlotChrome::Full,
            Self::NoGrid => PlotChrome::NoGrid,
            Self::TracesOnly => PlotChrome::Bare,
        }
    }

    fn is_full(&self) -> bool {
        *self == Self::Full
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AxisFile {
    Row,
    Column,
}

/// A node of the pane tree: a leaf names its pane, a split its two halves.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum NodeFile {
    Leaf {
        pane: u32,
    },
    Split {
        split: AxisFile,
        ratio: f32,
        a: Box<NodeFile>,
        b: Box<NodeFile>,
    },
}

impl NodeFile {
    fn of(n: &PaneNode) -> Self {
        match n {
            PaneNode::Leaf(id) => Self::Leaf { pane: id.0 },
            PaneNode::Split { axis, ratio, a, b } => Self::Split {
                split: match axis {
                    Axis::Row => AxisFile::Row,
                    Axis::Column => AxisFile::Column,
                },
                ratio: *ratio,
                a: Box::new(Self::of(a)),
                b: Box::new(Self::of(b)),
            },
        }
    }

    /// The tree; `None` when a ratio is not a number (NaN passes a clamp, and a split by
    /// it has no place for either side).
    fn node(&self) -> Option<PaneNode> {
        Some(match self {
            Self::Leaf { pane } => PaneNode::Leaf(PaneId(*pane)),
            Self::Split { split, ratio, a, b } => PaneNode::Split {
                axis: match split {
                    AxisFile::Row => Axis::Row,
                    AxisFile::Column => Axis::Column,
                },
                ratio: ratio.is_finite().then(|| ratio.clamp(0.0, 1.0))?,
                a: Box::new(a.node()?),
                b: Box::new(b.node()?),
            },
        })
    }
}

/// One pane: its kind, measurement and the modes of each kind it may show.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaneEntryFile {
    id: u32,
    kind: PaneFile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    measurement: Option<String>,
    #[serde(default)]
    transfer_view: TransferViewFile,
    #[serde(default = "spl_meter_leq")]
    spl_view: SplViewFile,
    #[serde(default)]
    spectrum_view: SpectrumViewFile,
    #[serde(default)]
    sweep_view: SweepViewFile,
    #[serde(default = "ir_linear")]
    ir_mode: IrModeFile,
    #[serde(default, skip_serializing_if = "ChromeFile::is_full")]
    chrome: ChromeFile,
}

impl PaneEntryFile {
    fn of(p: &PanePrefs) -> Self {
        let m = &p.modes;
        Self {
            id: p.id.0,
            kind: PaneFile::of(p.kind),
            measurement: p.measurement.clone(),
            transfer_view: match m.transfer {
                TransferView::Response => TransferViewFile::Response,
                TransferView::Phase => TransferViewFile::Phase,
                TransferView::Coherence => TransferViewFile::Coherence,
                TransferView::Ir => TransferViewFile::Ir,
            },
            spl_view: match m.spl {
                SplMode::Meter => SplViewFile::Meter,
                SplMode::Leq => SplViewFile::Leq,
                SplMode::MeterLeq => SplViewFile::MeterLeq,
                SplMode::Bands => SplViewFile::Bands,
            },
            spectrum_view: match m.spectrum {
                SpectrumMode::Spectrum => SpectrumViewFile::Spectrum,
                SpectrumMode::Split => SpectrumViewFile::SpectrumSpectrograph,
                SpectrumMode::Spectrograph => SpectrumViewFile::Spectrograph,
            },
            sweep_view: match m.sweep {
                SweepMode::Response => SweepViewFile::Response,
                SweepMode::Ir => SweepViewFile::Ir,
                SweepMode::Room => SweepViewFile::Room,
            },
            ir_mode: match m.ir {
                IrMode::Linear => IrModeFile::Linear,
                IrMode::Log => IrModeFile::Log,
                IrMode::Etc => IrModeFile::Etc,
            },
            chrome: ChromeFile::of(m.chrome),
        }
    }

    fn parse(&self) -> PanePrefs {
        PanePrefs {
            id: PaneId(self.id),
            kind: self.kind.pane(),
            measurement: self.measurement.clone(),
            modes: PaneModes {
                transfer: match self.transfer_view {
                    TransferViewFile::Response => TransferView::Response,
                    TransferViewFile::Phase => TransferView::Phase,
                    TransferViewFile::Coherence => TransferView::Coherence,
                    TransferViewFile::Ir => TransferView::Ir,
                },
                spl: match self.spl_view {
                    SplViewFile::Meter => SplMode::Meter,
                    SplViewFile::Leq => SplMode::Leq,
                    SplViewFile::MeterLeq => SplMode::MeterLeq,
                    SplViewFile::Bands => SplMode::Bands,
                },
                spectrum: match self.spectrum_view {
                    SpectrumViewFile::Spectrum => SpectrumMode::Spectrum,
                    SpectrumViewFile::SpectrumSpectrograph => SpectrumMode::Split,
                    SpectrumViewFile::Spectrograph => SpectrumMode::Spectrograph,
                },
                sweep: match self.sweep_view {
                    SweepViewFile::Response => SweepMode::Response,
                    SweepViewFile::Ir => SweepMode::Ir,
                    SweepViewFile::Room => SweepMode::Room,
                },
                ir: match self.ir_mode {
                    IrModeFile::Linear => IrMode::Linear,
                    IrModeFile::Log => IrMode::Log,
                    IrModeFile::Etc => IrMode::Etc,
                },
                chrome: self.chrome.chrome(),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutFile {
    /// The focused pane's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    focus: Option<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    maximized: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    fullscreen: bool,
    #[serde(default = "unit_db")]
    distortion_unit: UnitFile,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hidden: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    compared: Vec<String>,
    /// Last: tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tree: Option<NodeFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    panes: Vec<PaneEntryFile>,
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

/// The `[layout]` table if it parses, else none and a warning naming why.
fn lenient_layout<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<LayoutFile>, D::Error> {
    let v = toml::Value::deserialize(d)?;
    match LayoutFile::deserialize(v) {
        Ok(l) => Ok(Some(l)),
        Err(e) => {
            tracing::warn!("ui.toml: [layout] dropped, one pane instead: {e}");
            Ok(None)
        }
    }
}

impl LayoutFile {
    fn parse(self) -> LayoutPrefs {
        let panes = self.tree.as_ref().and_then(|t| {
            let Some(root) = t.node() else {
                tracing::warn!(
                    "ui.toml: pane tree dropped, one pane instead: a ratio is not a number"
                );
                return None;
            };
            let leaves = root.leaves();
            let mut seen = BTreeSet::new();
            // A pane twice in the tree has no place to be drawn, and two entries of one id
            // say two things of one pane: either way the tree is dropped.
            if !leaves.iter().all(|id| seen.insert(*id)) {
                tracing::warn!(
                    "ui.toml: pane tree dropped, one pane instead: a pane twice in the tree"
                );
                return None;
            }
            let mut seen = BTreeSet::new();
            if !self.panes.iter().all(|p| seen.insert(p.id)) {
                tracing::warn!(
                    "ui.toml: pane tree dropped, one pane instead: two [[layout.panes]] of one id"
                );
                return None;
            }
            let views: Vec<PanePrefs> = self
                .panes
                .iter()
                .map(PaneEntryFile::parse)
                .filter(|p| leaves.contains(&p.id))
                .collect();
            Some(PanesPrefs {
                focus: self.focus.map_or(leaves[0], PaneId),
                root,
                views,
            })
        });
        LayoutPrefs {
            panes,
            maximized: self.maximized,
            fullscreen: self.fullscreen,
            distortion_unit: match self.distortion_unit {
                UnitFile::Db => DistortionUnit::Db,
                UnitFile::Percent => DistortionUnit::Percent,
            },
            hidden: self.hidden.into_iter().collect(),
            compared: self.compared.into_iter().collect(),
        }
    }

    fn from_prefs(l: &LayoutPrefs) -> Self {
        Self {
            focus: l.panes.as_ref().map(|p| p.focus.0),
            maximized: l.maximized,
            fullscreen: l.fullscreen,
            distortion_unit: match l.distortion_unit {
                DistortionUnit::Db => UnitFile::Db,
                DistortionUnit::Percent => UnitFile::Percent,
            },
            hidden: l.hidden.iter().cloned().collect(),
            compared: l.compared.iter().cloned().collect(),
            tree: l.panes.as_ref().map(|p| NodeFile::of(&p.root)),
            panes: l
                .panes
                .as_ref()
                .map(|p| p.views.iter().map(PaneEntryFile::of).collect())
                .unwrap_or_default(),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_device: Option<String>,
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
            output_device: self.output_device.clone(),
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
            output_device: r.output_device.clone(),
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
        for key in f.unknown.keys() {
            tracing::warn!("ui.toml: unknown setting `{key}` ignored");
        }
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
            warning_toasts: f.warning_toasts.unwrap_or(true),
            resolution_marker: f.resolution_marker.unwrap_or(true),
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
            legend: f
                .legend
                .as_ref()
                .and_then(|l| l.transfer.as_ref())
                .map(LegendFile::parse)
                .transpose()?
                .unwrap_or_default(),
        })
    }

    /// The file text.
    pub fn to_toml(&self) -> String {
        let f = File {
            key_hints: (!self.key_hints).then_some(false),
            unknown: BTreeMap::new(),
            warning_toasts: (!self.warning_toasts).then_some(false),
            resolution_marker: (!self.resolution_marker).then_some(false),
            spl_hold_ms: self.spl_hold_ms,
            theme: self.theme.map(ThemeFile::of),
            record_limit_min: self.record_limit_min,
            spectrograph_span_s: self.spectrograph_span_s,
            layout: (self.layout != LayoutPrefs::default())
                .then(|| LayoutFile::from_prefs(&self.layout)),
            levels: LevelsFile::from_prefs(&self.levels),
            legend: LegendFile::from_prefs(&self.legend),
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
                output_device: Some("hw:Out".into()),
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
    fn an_unknown_setting_is_dropped_and_the_rest_kept() {
        let p = UiPrefs::from_toml("key_hints = false\nno_such_setting = true\n").expect("loads");
        assert!(!p.key_hints);
        assert!(!p.to_toml().contains("no_such_setting"));
    }

    #[test]
    fn warning_toasts_round_trip() {
        let mut p = UiPrefs::default();
        assert!(p.warning_toasts);
        // On is the default and is not written.
        assert!(!p.to_toml().contains("warning_toasts"));
        p.warning_toasts = false;
        let text = p.to_toml();
        assert!(text.contains("warning_toasts = false"), "{text}");
        assert_eq!(UiPrefs::from_toml(&text), Ok(p));
        assert!(UiPrefs::from_toml("warning_toasts = \"off\"\n").is_err());
    }

    #[test]
    fn resolution_marker_round_trip() {
        let mut p = UiPrefs::default();
        assert!(p.resolution_marker);
        assert!(!p.to_toml().contains("resolution_marker"));
        p.resolution_marker = false;
        let text = p.to_toml();
        assert!(text.contains("resolution_marker = false"), "{text}");
        assert_eq!(UiPrefs::from_toml(&text), Ok(p));
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

    /// Two panes side by side: a transfer pane on `Main L` and an SPL pane on `FOH SPL`
    /// showing the Leq windows, the SPL pane focused.
    fn two_panes() -> PanesPrefs {
        PanesPrefs {
            root: PaneNode::Split {
                axis: Axis::Row,
                ratio: 0.5,
                a: Box::new(PaneNode::Leaf(PaneId(1))),
                b: Box::new(PaneNode::Leaf(PaneId(2))),
            },
            focus: PaneId(2),
            views: vec![
                PanePrefs {
                    id: PaneId(1),
                    kind: PaneKind::Transfer,
                    measurement: Some("Main L".to_owned()),
                    modes: PaneModes {
                        transfer: TransferView::Ir,
                        ..PaneModes::default()
                    },
                },
                PanePrefs {
                    id: PaneId(2),
                    kind: PaneKind::Spl,
                    measurement: Some("FOH SPL".to_owned()),
                    modes: PaneModes {
                        transfer: TransferView::Coherence,
                        spl: SplMode::Leq,
                        spectrum: SpectrumMode::Spectrograph,
                        sweep: SweepMode::Room,
                        ir: IrMode::Etc,
                        chrome: PlotChrome::Bare,
                    },
                },
            ],
        }
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
            panes: Some(two_panes()),
            maximized: true,
            fullscreen: true,
            distortion_unit: DistortionUnit::Percent,
            hidden: ["TF 2".to_owned()].into(),
            compared: ["Main R".to_owned()].into(),
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
            "focus = 2",
            "fullscreen = true",
            "distortion_unit = \"percent\"",
            "hidden = [\"TF 2\"]",
            "compared = [\"Main R\"]",
            "[layout.tree]",
            "split = \"row\"",
            "[[layout.panes]]",
            "kind = \"spl\"",
            "measurement = \"FOH SPL\"",
            "spl_view = \"leq\"",
            "spectrum_view = \"spectrograph\"",
            "sweep_view = \"room\"",
            "ir_mode = \"etc\"",
            "chrome = \"traces_only\"",
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
        for bad in [
            "spl_hold_ms = 5\n",
            "spl_hold_ms = 20000\n",
            "[window]\nwidth = 100\nheight = 100\n",
        ] {
            assert!(UiPrefs::from_toml(bad).is_err(), "{bad}");
        }
    }

    /// A layout that is not a pane tree — tables of panes by kind, a pane twice in the tree,
    /// an unknown view, a ratio not a number, two entries of one pane — is dropped: the rest
    /// of the file stays and the app starts with one pane.
    #[test]
    fn an_unreadable_layout_starts_with_one_pane() {
        for old in [
            "[layout]\nfocus = \"spl\"\nspl_view = \"leq\"\n\n[layout.measurements]\nspl = \"FOH\"\n",
            "[layout]\nfocus = 1\n\n[layout.tree]\nsplit = \"row\"\nratio = 0.5\na = { pane = 1 }\nb = { pane = 1 }\n",
            "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"spl\"\nspl_view = \"bars\"\n",
            "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"spl\"\nchrome = \"bare\"\n",
            "[layout]\nfocus = 1\n\n[layout.tree]\nsplit = \"row\"\nratio = nan\na = { pane = 1 }\nb = { pane = 2 }\n",
            "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"spl\"\n\n[[layout.panes]]\nid = 1\nkind = \"transfer\"\n",
        ] {
            let text = format!("spl_hold_ms = 250\n{old}");
            let q = UiPrefs::from_toml(&text).expect(&text);
            assert_eq!(q.layout.panes, None, "{old}");
            assert_eq!(q.spl_hold_ms, Some(250));
        }
        // A pane of the tree without its entry shows a transfer pane: the tree stays.
        let q =
            UiPrefs::from_toml("[layout]\nfocus = 1\n\n[layout.tree]\npane = 1\n").expect("parse");
        let panes = q.layout.panes.expect("tree");
        assert_eq!(panes.root, PaneNode::Leaf(PaneId(1)));
        assert!(panes.views.is_empty());
    }

    /// Each pane keeps its own views, written by name; a pane written without them takes
    /// the defaults (the meter with the Leq windows, the spectrum).
    #[test]
    fn pane_views_default_and_keep_a_choice() {
        assert_eq!(PaneModes::default().spl, SplMode::MeterLeq);
        assert_eq!(PaneModes::default().spectrum, SpectrumMode::Spectrum);
        let q = UiPrefs::from_toml(
            "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"spl\"\n",
        )
        .expect("parse");
        let v = &q.layout.panes.expect("tree").views[0];
        assert_eq!((v.kind, v.modes), (PaneKind::Spl, PaneModes::default()));
        for (spl, name) in [
            (SplMode::Meter, "meter"),
            (SplMode::Leq, "leq"),
            (SplMode::MeterLeq, "meter_leq"),
            (SplMode::Bands, "bands"),
        ] {
            let mut p = UiPrefs::default();
            let mut panes = two_panes();
            panes.views[1].modes.spl = spl;
            p.layout.panes = Some(panes);
            let text = p.to_toml();
            assert!(text.contains(&format!("spl_view = \"{name}\"")), "{text}");
            assert_eq!(UiPrefs::from_toml(&text).expect("parse"), p);
        }
        for (mode, name) in [
            (SpectrumMode::Spectrum, "spectrum"),
            (SpectrumMode::Split, "spectrum_spectrograph"),
            (SpectrumMode::Spectrograph, "spectrograph"),
        ] {
            let mut p = UiPrefs::default();
            let mut panes = two_panes();
            panes.views[0].kind = PaneKind::Spectrum;
            panes.views[0].modes.spectrum = mode;
            p.layout.panes = Some(panes);
            let text = p.to_toml();
            assert!(
                text.contains(&format!("spectrum_view = \"{name}\"")),
                "{text}"
            );
            assert_eq!(UiPrefs::from_toml(&text).expect("parse"), p);
        }
    }

    #[test]
    fn bad_files_are_reported() {
        assert!(UiPrefs::from_toml("[stimulus_outputs]\nx = [0]\n").is_err());
        assert!(UiPrefs::from_toml("[stimulus_outputs]\nx = []\n").is_err());
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

    /// A legend moved or resized is written, its defaults left out; out of bounds is an
    /// error.
    #[test]
    fn legend_round_trip() {
        assert!(!UiPrefs::default().to_toml().contains("legend"));
        let mut p = UiPrefs::default();
        p.legend.x = 1.0;
        p.legend.max_height = 0.3;
        let text = p.to_toml();
        assert!(
            text.contains("[legend.transfer]\nx = 1.0\nmax_height = 0.3\n"),
            "{text}"
        );
        let back = UiPrefs::from_toml(&text).expect("parse");
        assert_eq!(back.legend.x, 1.0);
        assert_eq!(back.legend.max_height, 0.3);
        assert_eq!(back.legend.y, 0.0);
        assert!(UiPrefs::from_toml("[legend.transfer]\nx = 2.0\n").is_err());
        assert!(UiPrefs::from_toml("[legend.transfer]\nmax_width = 0.01\n").is_err());
        assert!(UiPrefs::from_toml("[legend.spectrum]\nx = 0.5\n").is_err());
        let hidden = UiPrefs::from_toml("[legend.transfer]\nhidden = true\n").expect("parse");
        assert!(hidden.legend.hidden);
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
