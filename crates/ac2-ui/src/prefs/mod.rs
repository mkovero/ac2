//! UI preferences kept between runs (`ui.toml` in the ac2 config directory): the stimulus
//! outputs last used on each output device (decision K4), the session dialog's choices
//! per device — which inputs and outputs were in the session, their roles and the mic
//! names — the Leq view's layout, whether the panes show their key hints, how long the SPL
//! meter's number holds a reading, the theme, the record toggle's time limit, the layout
//! and window as last left: the pane tree, the
//! focused pane, maximised or full screen, what each pane shows (its kind, measurement,
//! modes and how much of its plot: grid, labels, cursor, its own level axis range: a fit
//! made for one show is a fair start for the next, and where its plates were dropped and
//! their text size), the window's size and position, the impulse-response views' dB axes,
//! whether the transfer pane's legend is hidden, and the session files
//! this app opened or exported (the Sessions dialog's recents).
//!
//! ```toml
//! key_hints = false
//! spl_hold_ms = 250
//! theme = "light"
//! record_limit_min = 90
//!
//! [stimulus_outputs]
//! "hw:UMC1820" = [1, 2]
//!
//! [sessions."coreaudio/hw:UMC1820"]
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
//! [layout.panes.levels]
//! transfer = [-24.0, 12.0]
//!
//! [layout.panes.plates.readout]
//! at = [0.0, 1.0]
//! scale = 2.0
//!
//! [[layout.panes]]
//! id = 2
//! kind = "spl"
//! measurement = "FOH SPL"
//! spl_view = "meter_leq"
//! chrome = "no_grid"
//!
//! [ir_levels]
//! transfer = [-80.0, 3.0]
//! sweep = [-90.0, 0.0]
//!
//! [legend.transfer]
//! hidden = true
//!
//! [window]
//! width = 1600
//! height = 900
//! x = 80
//! y = 40
//!
//! [[recent_sessions]]
//! path = "/run/media/foh/STICK/friday.ac2session"
//! opened_at = 1789500000000000000
//! ```
//!
//! Channels are one-based in the file, as everywhere the operator reads or types them.
//! Measurements are remembered by name: ids do not outlive the daemon's state. The file is
//! UI state, not configuration: one that cannot be read is reported once and replaced by
//! the next save, and a write is atomic (`ac2_paths::write_private_atomic`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ac2_proto::model::layout::{LayoutLevels, LevelRange};
use ac2_scene::axis::Range;
use ac2_scene::legend::LegendView;
use ac2_scene::plate::{PlateKind, PlatePlace, Plates};
use ac2_scene::view::{
    DistortionUnit, IrMode, LeqLayout, LeqStyle, PlotChrome, SpectrumMode, SplMode, SweepMode,
    ViewState, level,
};
use serde::{Deserialize, Serialize};

use crate::state::{Axis, PaneId, PaneKind, PaneModes, PaneNode, TransferView};

mod file;
mod layout;
#[cfg(test)]
mod tests;

/// An input's remembered kind: `InputKind` without the mic's name and curve (the name is in
/// [`DeviceRoles::mic_names`], the curve is the daemon's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KindPref {
    Mic,
    Line,
    Digital,
}

impl KindPref {
    /// The kind of a declared input.
    pub fn of(kind: &ac2_proto::model::InputKind) -> Self {
        use ac2_proto::model::InputKind as K;
        match kind {
            K::Mic { .. } => Self::Mic,
            K::Line => Self::Line,
            K::Digital => Self::Digital,
        }
    }
}

/// The session dialog's choices for one device (zero-based channels).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceRoles {
    /// Inputs in the session.
    pub inputs: Vec<u16>,
    /// Output channels of the session (a count: outputs `0 .. outputs`).
    pub outputs: u16,
    /// Spare inputs the session opens (JACK); listed in `inputs` like any other.
    pub spare_inputs: u16,
    /// The loopback return.
    pub reference: Option<u16>,
    /// Measurement mics.
    pub mics: Vec<u16>,
    /// Stimulus outputs.
    pub stimulus: Vec<u16>,
    /// Mic name per input.
    pub mic_names: BTreeMap<u16, String>,
    /// The kind declared per input. An input absent here is a mic when it is in `mics`
    /// (a file from before kinds names its mics there), else line.
    pub kinds: BTreeMap<u16, KindPref>,
    /// Left/right pairs among the inputs.
    pub pairs: Vec<ac2_proto::model::Pair>,
    /// The device the session plays on when it is not the input device (its id); `None`:
    /// the input device's own outputs, or the system default where it has none.
    pub output_device: Option<String>,
}

impl DeviceRoles {
    /// The kind remembered for `channel`: declared, else a mic when listed among the mics
    /// (an unnamed mic keeps its SPL meters), else line.
    pub fn kind_of(&self, channel: u16) -> KindPref {
        self.kinds
            .get(&channel)
            .copied()
            .unwrap_or(if self.mics.contains(&channel) {
                KindPref::Mic
            } else {
                KindPref::Line
            })
    }
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
    /// With `fullscreen`: the panes alone, no top bar or lists (F's second step).
    pub panes_only: bool,
    pub distortion_unit: DistortionUnit,
    /// Measurements whose live curves are hidden (A), by name.
    pub hidden: BTreeSet<String>,
}

impl Default for LayoutPrefs {
    fn default() -> Self {
        Self {
            panes: None,
            maximized: false,
            fullscreen: false,
            panes_only: false,
            distortion_unit: DistortionUnit::Db,
            hidden: BTreeSet::new(),
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
    /// What a Programme pane metered.
    pub meters: Option<crate::state::Metered>,
    pub modes: PaneModes,
    /// The level axis ranges the operator set on it (one not set is framed when the pane
    /// first shows a curve).
    pub levels: LayoutLevels,
    /// Where its plates were dropped and their text size.
    pub plates: ac2_scene::plate::Plates,
}

/// The impulse-response views' dB axes (log / ETC), dB re peak: one for the transfer
/// panes' IR views, one for the sweep panes'. The panes' level axes are each pane's own
/// ([`PanePrefs::levels`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IrLevelPrefs {
    pub transfer: Range,
    pub sweep: Range,
}

impl IrLevelPrefs {
    /// The ranges `view` shows.
    pub fn of(view: &ViewState) -> Self {
        Self {
            transfer: view.ir.axes.level_db,
            sweep: view.distortion.ir.level_db,
        }
    }

    /// Puts the ranges on `view`.
    pub fn apply(&self, view: &mut ViewState) {
        view.ir.axes.level_db = self.transfer;
        view.distortion.ir.level_db = self.sweep;
    }
}

impl Default for IrLevelPrefs {
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
    /// What shows through a plate the pointer is not on (see-through until the operator
    /// sets it solid).
    pub plate_background: ac2_scene::plate::PlateBackground,
    /// The Leq and band Leq bars glide to each new reading (on until the operator turns it
    /// off); off, they jump to it as received.
    pub bar_ease: bool,
    /// The live spectrum and RTA curves glide to each new frame (on until the operator
    /// turns it off); off, they jump to it as received.
    pub spec_ease: bool,
    /// How far a peak or notch must stand out for the cursor to jump to it, dB
    /// (`ac2_scene::cursor_jump`).
    pub jump_prominence_db: f64,
    /// How long the SPL meter's number holds a reading; `None`: by its time weighting
    /// (`ac2_scene::spl::display_period_s`).
    pub spl_hold_ms: Option<u32>,
    /// The goniometer embers' cooling time constant, seconds
    /// (`ac2_scene::gonio::EMBER_DECAY_RANGE_S`).
    pub ember_decay_s: f32,
    /// The goniometer embers' glow, a gain on landing intensity
    /// (`ac2_scene::gonio::EMBER_GLOW_RANGE`).
    pub ember_glow: f32,
    /// The transfer panes' headroom advice: on or off and its thresholds.
    pub advice: ac2_scene::advice::AdviceSettings,
    /// The layout as last left.
    pub layout: LayoutPrefs,
    /// The impulse-response dB axes as last left.
    pub ir_levels: IrLevelPrefs,
    /// The window as last left (`None`: never saved).
    pub window: Option<WindowPrefs>,
    /// The theme last chosen (`None`: never chosen; `ac2-ui --theme` overrides it for a run).
    pub theme: Option<ac2_scene::theme::ThemeName>,
    /// How long the record toggle records before it stops by itself, minutes (`None`: an
    /// hour, [`crate::state::RECORD_MAX_S`]).
    pub record_limit_min: Option<u32>,
    /// The transfer pane's legend as last left.
    pub legend: LegendPrefs,
    /// Session files and folders on this machine the app opened or exported, most recent
    /// first, at most [`MAX_RECENT_SESSIONS`].
    pub recent_sessions: Vec<RecentSession>,
}

/// How many session files the recents keep.
pub const MAX_RECENT_SESSIONS: usize = 12;

/// A session file or folder on this machine the app opened or exported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentSession {
    pub path: String,
    /// When (this machine's wall clock, ns).
    pub opened_at: u64,
}

impl UiPrefs {
    /// `path` used now: first in the recents, once.
    pub fn note_recent_session(&mut self, path: &str, at: ac2_proto::units::WallNs) {
        self.recent_sessions.retain(|r| r.path != path);
        self.recent_sessions.insert(
            0,
            RecentSession {
                path: path.to_owned(),
                opened_at: at.0,
            },
        );
        self.recent_sessions.truncate(MAX_RECENT_SESSIONS);
    }
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
            plate_background: ac2_scene::plate::PlateBackground::SeeThrough,
            bar_ease: true,
            spec_ease: true,
            jump_prominence_db: ac2_scene::cursor_jump::DEFAULT_PROMINENCE_DB,
            spl_hold_ms: None,
            ember_decay_s: ac2_scene::gonio::EMBER_DECAY_DEFAULT_S,
            ember_glow: ac2_scene::gonio::EMBER_GLOW_DEFAULT,
            advice: ac2_scene::advice::AdviceSettings::default(),
            layout: LayoutPrefs::default(),
            ir_levels: IrLevelPrefs::default(),
            window: None,
            theme: None,
            record_limit_min: None,
            legend: LegendPrefs::default(),
            recent_sessions: Vec::new(),
        }
    }
}

/// A legend as last left: hidden or not (where it sits and its text size are each pane's,
/// [`PanePrefs::plates`]; the scroll is not kept).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LegendPrefs {
    pub hidden: bool,
}

impl LegendPrefs {
    pub fn of(v: &LegendView) -> Self {
        Self { hidden: v.hidden }
    }

    /// Puts the choices on `v`.
    pub fn apply(&self, v: &mut LegendView) {
        v.hidden = self.hidden;
    }
}
